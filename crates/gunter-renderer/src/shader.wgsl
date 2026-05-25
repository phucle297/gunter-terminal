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
};

struct FragIn {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) bg: vec3<f32>,
    @location(2) fg: vec3<f32>,
};

@vertex
fn vs_main(v: VertIn, i: InstIn) -> FragIn {
    let px = (i.cell_pos.x + v.pos.x) * uniforms.cell_size.x;
    let py = (i.cell_pos.y + v.pos.y) * uniforms.cell_size.y;
    let cx = (px / uniforms.viewport.x) * 2.0 - 1.0;
    let cy = 1.0 - (py / uniforms.viewport.y) * 2.0;

    var out: FragIn;
    out.clip_pos = vec4<f32>(cx, cy, 0.0, 1.0);
    out.uv = mix(i.uv_min, i.uv_max, v.pos);
    out.bg = i.bg;
    out.fg = i.fg;
    return out;
}

@fragment
fn fs_main(in: FragIn) -> @location(0) vec4<f32> {
    let alpha = textureSample(atlas_tex, atlas_samp, in.uv).r;
    let color = mix(in.bg, in.fg, alpha);
    return vec4<f32>(color, 1.0);
}
