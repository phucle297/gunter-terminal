# Gunter Renderer — Implementation Notes

See SPEC.md Phase 2 for pipeline overview and GlyphInstance layout.

---

## wgpu Initialization

Prefer DX12 on Windows; fall back to Vulkan. Use `Fxc` to avoid runtime dependency on `dxcompiler.dll`:

```rust
let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
    backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
    dx12_shader_compiler: wgpu::Dx12Compiler::Fxc,
    ..Default::default()
});
```

Surface format: prefer `TextureFormat::Bgra8UnormSrgb` (GPU encodes linear→sRGB on write, no manual gamma in shaders). Check adapter support via `adapter.get_texture_format_features`; fall back to `Rgba8Unorm` + manual gamma if unavailable.

Present mode: `PresentMode::AutoVsync` (FIFO on Vulkan, Flip+VSync on DX12).

---

## WGSL Shaders

**Background pass — vertex shader**

```wgsl
struct InstanceInput {
    @location(0) pos:  vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) bg:   vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_bg(
    @builtin(vertex_index) vi: u32,
    instance: InstanceInput,
) -> VertexOutput {
    let unit = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = instance.pos + unit * instance.size;
    let ndc = vec2<f32>(
        (px.x / viewport.width)  * 2.0 - 1.0,
        1.0 - (px.y / viewport.height) * 2.0,
    );
    return VertexOutput(vec4<f32>(ndc, 0.0, 1.0), instance.bg);
}
```

No matrix multiply needed — pixel→NDC is two FMAs per axis.

**Glyph pass — fragment shader**

```wgsl
@fragment
fn fs_glyph(in: GlyphVertexOutput) -> @location(0) vec4<f32> {
    let s = textureSample(atlas, atlas_sampler, in.uv);
    // s.a = grayscale coverage; premultiplied alpha blend
    return in.fg * s.a;
}
```

For colored emoji (RGBA atlas glyphs): return `s` directly, ignoring `fg`. Detect at instance-build time (set a flag or batch separately).

No subpixel/ClearType rendering: fails on composited windows and RDP. Grayscale AA only.

---

## Dirty-Cell Upload Strategy

```
Each frame:
  1. If grid.all_dirty: upload entire instance buffer, clear flag, done.
  2. Else: walk grid.dirty Vec<bool>, collect dirty indices.
  3. Merge adjacent indices into contiguous ranges.
  4. For each range [start, end]:
       queue.write_buffer(&buf, start * INSTANCE_SIZE, &instances[start..end])
  5. Clear dirty flags.
```

Cursor blink = one 40-byte write. Full clear = 11,000 × 40 B = 440 KB — still fast with a persistent staging buffer.

Set `all_dirty = true` on: resize, font reload, theme change, `erase_in_display`, scroll events, palette change.

---

## Glyph Atlas

**Texture**: `RGBA8Unorm`, 2048×2048 default. RGBA (not R8 grayscale) to host colored emoji and monochrome glyphs in one texture. Cost: 16 MB VRAM — acceptable.

**Allocator**: shelf packer — O(1) insert, no repack. Rows of fixed height (tallest glyph so far); new glyph appended to current row if it fits, else new row.

**Overflow**: allocate a second atlas page. `GlyphKey` maps to `(page_index, uv_rect)`. The instance buffer carries `page_index` if multi-page; alternatively use a `Texture2dArray`.

**Warmup**: at startup rasterize U+0020..U+007E × {normal, bold, italic, bold+italic} for the configured font + fallback chain. Covers ~95% of a typical session.

**Lazy fill**: on first encounter of a glyph outside warmup, rasterize and `write_texture` to patch the atlas. No full re-upload.

**Nerd Font / private use area (U+E000–U+F8FF)**: these are common in powerline prompts and must be in the atlas. `wezterm-font` handles them natively. Do not skip U+E000–U+F8FF in the lazy path.

**DPI change**: when the window moves to a monitor with a different scale factor, rebuild the entire atlas at the new `font_size_px * dpi_scale`. Set `all_dirty = true` on all grids.

---

## ConPTY Resize Sequence

On winit `Resized` event:

1. Recompute `(cols, rows)` from new pixel size ÷ cell size.
2. `pty.resize(PtySize { cols, rows, pixel_width, pixel_height })` — set pixel fields (not zero); ConPTY uses them for DPI-aware layout inside the PTY.
3. Reflow layout tree → update each session's `rect`.
4. Set `all_dirty = true` on all grids.

`portable-pty` sends `SIGWINCH` on Unix automatically; ConPTY handles it on Windows.
