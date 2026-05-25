# Gunter Renderer Subsystem — Design Document

> Crate: `gunter-renderer`
> Last updated: 2026-05-25

---

## Summary

The renderer is the highest-complexity subsystem in Gunter. A terminal at 60fps with thousands of cells per frame cannot tolerate per-cell draw calls, DOM overhead, or CPU rasterization at draw time. Every decision below is anchored to three constraints:

1. **Throughput**: a 220×50 grid has 11,000 cells. The renderer must handle a full-dirty frame (e.g. `clear && ls -laR`) without dropping below 60fps.
2. **Correctness**: text must look correct across DPI scales, font variants (bold/italic), and Nerd Font private-use glyphs.
3. **Portability**: the primary target is WSL2 on Windows, but the design must not foreclose native Linux or macOS later.

The architecture is: **one glyph atlas texture built at startup, two instanced draw calls per frame (background quads + glyph quads), with per-cell dirty tracking to minimize vertex buffer uploads.**

---

## 1. Rendering Backend — wgpu

### Decision

Use `wgpu` as the sole GPU abstraction layer. Target the DX12 backend on Windows (WSL2 native) and Vulkan on Linux. Metal on macOS is a future concern.

### Why wgpu wins

`wgpu` is the correct choice for this project along five dimensions:

**a) Single-codebase multi-backend**
`wgpu` translates WGSL shaders and a Rust API to DX12, Vulkan, Metal, or WebGPU at runtime. Writing one renderer that targets DX12 on Windows and Vulkan on Linux is zero extra work. The alternatives require either branching shader codebases or sacrificing one platform.

**b) Safety without raw `unsafe` sprawl**
`wgpu` exposes a safe Rust API. Vulkan (`ash`) and OpenGL (raw FFI) require extensive `unsafe` blocks. For a terminal emulator that is already juggling PTY threads, VTE parsing, and async I/O, reducing `unsafe` surface area is a real reliability benefit — not just a style preference.

**c) Active ecosystem and tooling**
`wgpu` is the renderer backend for Bevy, wgpu-rs examples are extensive, and `wgpu`'s validation layer (available in debug builds) catches API misuse before it becomes a hard GPU crash. The alternatives either have stale ecosystems (glium), thin community (vulkano), or are too low-level for the iteration speed this project needs (ash).

**d) WSL2 compatibility**
WSL2 supports GPU via `wsl-opengl` compatibility layers and also via native DX12 through Mesa's Dozen or WSLg. `wgpu` on DX12 works with WSLg's GPU paravirtualization. Raw Vulkan on WSL2 requires the user to have a Vulkan-capable driver and Vulkan layers correctly installed — a fragile dependency. DX12 via `wgpu` on the Windows side (running the binary natively under Windows pointing at WSL2 shell) is the most reliable path.

**e) `wezterm-font` compatibility**
WezTerm itself uses `wgpu`. Vendoring or adapting the font layer from WezTerm is substantially easier when the GPU abstraction matches.

### Why not the alternatives

**OpenGL / glium**
OpenGL is a 1992 API with decades of driver-specific bugs, implicit global state, and no validation layer. `glium` is safe Rust but is largely unmaintained (last meaningful release 2022). On macOS, Apple deprecated OpenGL in 2018 — it still works but receives no driver updates. Reject.

**softbuffer**
CPU rasterization into a framebuffer. Correct for a low-power fallback but fundamentally incompatible with the "GPU rendering" design goal. A 4K display at 60fps at 4 bytes per pixel = 3.7 GB/s of CPU-to-display memory bandwidth. Reject.

**vulkano**
Higher-level Vulkan wrapper in Rust. More maintained than glium but still requires explicit render passes, sync primitives, and pipeline management at a level that does not buy us anything over `wgpu`. The `wgpu` abstraction handles all of this more ergonomically with zero portability loss for this project's targets. Reject.

**ash (raw Vulkan)**
Maximum control, maximum danger. Appropriate for a game engine or a driver-level tool. For a terminal emulator, the setup cost (1000+ lines of boilerplate before drawing a triangle) and `unsafe` surface area are not justified. Reject.

### Trade-offs accepted

- `wgpu` adds ~3 MB to the binary (vs ~1 MB for softbuffer). Acceptable against the 15 MB binary target.
- `wgpu`'s validation layer has non-trivial CPU overhead in debug builds. Disable in release.
- `wgpu` version lock: major breaking changes occur. Pin to a specific minor version and upgrade deliberately.

---

## 2. Glyph Atlas Strategy — wezterm-font

### Decision

Use `wezterm-font` (vendored or as a crate dependency) for glyph rasterization and atlas packing. Atlas is a single `wgpu::Texture` (RGBA8, 2048×2048 default, growable). UV rects stored in a `HashMap<(char, CellFlags), UvRect>`.

Atlas packing uses a shelf/row allocator (next-fit decreasing height). New glyphs are appended during startup warmup and lazily on first encounter.

### Why wezterm-font

`wezterm-font` is a battle-tested font subsystem that handles:

- Multi-font fallback chains (primary font → Nerd Font → CJK fallback → emoji)
- Nerd Font private use area glyphs (U+E000–U+F8FF) — critical for powerline-style prompts
- HarfBuzz shaping for ligatures (optional but present)
- FreeType + CoreText + DirectWrite backends depending on platform
- Correct handling of bold/italic synthetic styles when the font family does not include a dedicated variant

Writing any of this from scratch is a multi-week project. WezTerm spent years getting it right. The only cost is vendoring a crate that was not designed as a standalone library, but that is an engineering task, not a design risk.

### Why not the alternatives

**fontdue**
Pure Rust, no system dependencies, very fast CPU rasterization. Excellent for small embedded use cases. Does not handle HarfBuzz shaping, has limited fallback support, and Nerd Font composite glyphs (multi-codepoint sequences) are not handled. Reject for a full-featured terminal.

**ab_glyph**
Parses TTF/OTF in pure Rust. Similar limitations to fontdue — no shaping, no fallback chain, no complex script support. Good for simple UI labels, wrong for a terminal. Reject.

**rusttype**
Predecessor to `ab_glyph`, now unmaintained. Reject.

**FreeType bindings (freetype-rs)**
FreeType is the correct rasterizer and `wezterm-font` wraps it on Linux. Using `freetype-rs` directly means reimplementing fallback, shaping, atlas management, and platform detection that `wezterm-font` already provides. Only do this if vendoring `wezterm-font` proves impossible. Avoid.

**cosmic-text**
Modern pure-Rust text layout from System76, using `rustybuzz` (HarfBuzz port) for shaping. A legitimate alternative with good momentum. The reason to prefer `wezterm-font` is that WezTerm has solved every terminal-specific edge case already (Nerd Font glyphs, box-drawing characters, wide/half-width CJK). `cosmic-text` is optimized for general document layout. Reconsider if `wezterm-font` vendoring proves too costly.

### Atlas packing approach

```
Atlas: wgpu::Texture (RGBA8Unorm, 2048×2048)
Allocator: shelf packer
  - Rows of fixed height (= tallest glyph in the row so far)
  - New glyph fits on current row if width fits; else new row
  - On overflow: allocate a second texture (atlas page 1, 2, ...)
  - GlyphKey maps to (page_index, uv_rect)

Warmup: at startup, rasterize printable ASCII (U+0020..U+007E)
  for all combinations of CellFlags::BOLD | ITALIC
  for the configured font + fallback chain.
  This fills ~95% of what a typical terminal session uses.

Lazy fill: on first encounter of a glyph outside the warmup set
  (e.g. a Nerd Font icon), rasterize and append to atlas.
  Mark entire vertex buffer dirty for that cell.
```

The atlas texture is uploaded to the GPU once during warmup, then patched with `write_texture` for lazy additions. This avoids full texture re-uploads for common cases.

### Trade-offs accepted

- `wezterm-font` vendoring adds build complexity. Justified by time savings.
- RGBA8 atlas (vs R8 grayscale) wastes 3x memory for monochrome glyphs. Accepted to unify colored emoji and monochrome glyphs in one texture. A 2048×2048 RGBA8 atlas is 16 MB — well within VRAM budgets.
- Shelf packing leaves up to ~30% wasted space vs optimal bin packing. Accepted because atlas size is not a bottleneck at this scale, and shelf packing is O(1) per insert with no repack needed.

---

## 3. Instanced Draw Call Design

### Decision

Use GPU instancing: one `wgpu::Buffer` (vertex buffer with instance step mode) containing one `GlyphInstance` per dirty cell, one `draw_indexed_indirect` or `draw_indexed` call per pass. Two passes total: background quads, then glyph quads.

`GlyphInstance` layout:

```rust
#[repr(C)]
pub struct GlyphInstance {
    pub pos:    [f32; 2],   // top-left pixel, screen space
    pub size:   [f32; 2],   // cell width × cell height in pixels
    pub uv_pos: [f32; 2],   // atlas UV top-left (0.0..1.0)
    pub uv_sz:  [f32; 2],   // atlas UV width × height
    pub fg:     [f32; 4],   // linear RGBA
    pub bg:     [f32; 4],   // linear RGBA (background pass uses this; glyph pass uses fg)
}
// Total: 10 * 4 = 40 bytes per instance
```

The base geometry is a unit quad (2 triangles, 6 indices or 4 vertices + index buffer). The vertex shader scales it to `size` and translates it to `pos`. The instance buffer is uploaded once per frame (only dirty cells).

### Why instancing

**vs. one draw call per cell**
11,000 draw calls per frame saturates the CPU-side GPU command encoder. Modern GPUs execute thousands of triangles per draw call efficiently; the overhead is in the command processing, not the rasterization. Instancing collapses 11,000 potential draw calls to 1. Non-negotiable.

**vs. geometry shaders**
Geometry shaders take a point primitive and emit a quad per glyph. This was a popular technique circa 2012. Problems: geometry shaders are the slowest programmable stage on modern GPUs (they serialize on most hardware), are deprecated in Metal (macOS), and are not available in WebGPU (and therefore not in `wgpu`'s portable subset. `wgpu` does not expose geometry shaders at all. Reject.

**vs. SSBO / storage buffer with a compute shader**
A compute shader could read a storage buffer of cell data and write directly to the framebuffer. This is the most GPU-friendly approach in theory (no vertex processing, coalesced writes). Problems: (a) requires compute + render synchronization barriers, (b) writing to a render target from compute is complex and not well-supported in `wgpu`'s current API surface, (c) significantly increases shader complexity for marginal gain at terminal grid scales. Revisit if profiling shows the vertex processing stage is a bottleneck (unlikely at 11k cells).

**vs. a texture-atlas quad per draw call (batch-blit)**
Some simple 2D renderers maintain a sorted quad list and flush them in a single draw when the texture changes. For a terminal with one atlas texture, this collapses to instancing anyway. The distinction is semantic, not functional.

**vs. indirect draw (draw_indexed_indirect)**
`draw_indexed_indirect` reads draw parameters from a GPU buffer, enabling the GPU to cull cells without CPU involvement. Useful for large grids where many cells are off-screen (e.g. during scrollback). Not needed at Phase 2; add in Phase 3+ if scrollback rendering shows CPU bottleneck.

### Instance buffer upload strategy

```
Each frame:
  1. Walk Grid::dirty Vec<bool>. Collect indices of dirty cells.
  2. For each dirty cell index i:
       glyph_instances[i] = build_glyph_instance(grid.cells[i], ...)
  3. Compute contiguous dirty ranges (merge adjacent dirty cells).
  4. For each range [start, end]:
       queue.write_buffer(&instance_buf, start * INSTANCE_SIZE, &instances[start..end])
  5. Clear dirty flags.
```

This means only dirty cells generate any buffer traffic. A cursor blink (one cell dirty) generates one 40-byte write. A full clear generates 11,000 × 40 = 440 KB write — still fast on DX12 with a staging buffer.

### Trade-offs accepted

- 40 bytes per instance is slightly larger than the absolute minimum (could drop `bg` from the glyph pass, saving 16 bytes). Kept for shader simplicity — one struct for both passes.
- Instancing requires a fixed instance count per draw. Cells that are "space with default background" still need a background instance. The glyph pass can skip them (uv_sz = 0 and the vertex shader can emit degenerate triangles). Add this optimization only after profiling.

---

## 4. Dirty Tracking

### Decision

Per-cell `Vec<bool>` dirty flags in `Grid` (one bool per cell, row-major). Set to `true` by the VTE performer on any cell write. Cleared by the renderer after upload. A separate `frame_dirty: bool` flag marks full redraws needed (resize, font reload, theme change).

### Design

```rust
pub struct Grid {
    pub dirty: Vec<bool>,  // len = cols * rows
    pub all_dirty: bool,   // if true, skip per-cell check and upload everything
}
```

On VTE `print` (character output): `grid.dirty[row * cols + col] = true`.
On VTE `erase_in_display` (clear screen): `grid.all_dirty = true`.
On resize: `grid.all_dirty = true`.

### Why per-cell dirty flags

**vs. full redraw every frame**
A full 220×50 grid upload is 440 KB of vertex buffer writes every frame regardless of what changed. At 60fps that is 26 MB/s of constant buffer uploads. Not catastrophic on modern hardware but wasteful and unnecessary. Dirty tracking reduces this to the actual changed cells.

**vs. damage regions (rectangular bounding box of dirty cells)**
Damage regions track the minimal axis-aligned bounding rectangle of all dirty cells per frame. Simpler to compute, but wasteful when changes are scattered (e.g. status bar update at row 0 + cursor blink at row 23 → entire frame dirty). Per-cell flags are strictly more precise.

**vs. dirty row flags**
Per-row dirty flags halve the dirty tracking overhead vs per-cell flags and are sufficient for scroll operations (which dirty entire rows at once). The trade-off: if only one cell in a 220-column row changes (cursor movement), per-row flags force uploading 220 instances instead of 1. Per-cell wins for cursor blink, which is the highest-frequency single-cell update.

**vs. double-buffering with memcmp**
Maintain a `prev_frame: Vec<Cell>` and diff against `grid.cells` each frame. Eliminates dirty flag writes from the VTE path (simpler VTE code) at the cost of a full memcmp per frame (11,000 × ~12 bytes = 132 KB of comparisons). At 60fps: 7.9 MB/s of reads just for diffing. Marginal cost but adds latency. Dirty flags win on correctness: they are set synchronously with cell writes, not detected one frame late.

### Memory cost

220 × 50 bools = 11,000 bytes ≈ 11 KB. Negligible. With `Vec<u8>` as a bitset it would be 1.4 KB. Use `Vec<bool>` for simplicity first; replace with a bitset if profiling shows cache misses from dirty-flag scans.

### Trade-offs accepted

- Dirty flags can become stale if a VTE sequence writes the same value twice without an intervening frame render. This is safe: the renderer will redundantly upload an unchanged instance. Not a correctness issue.
- `all_dirty = true` must be set on every operation that can change the meaning of cells without going through individual `print` calls: erase, scroll, resize, cursor style change, color palette change (theme reload).

---

## 5. Frame Timing — 60fps with vsync

### Decision

Use `wgpu::PresentMode::AutoVsync` (maps to `FIFO` on Vulkan, `Flip + VSync` on DX12). Target exactly 60fps. No adaptive sync, no frame pacing algorithm beyond the OS vsync mechanism.

### Why vsync

**vs. `PresentMode::Immediate` (no vsync)**
Immediate mode renders as fast as possible and may tear on displays that are not in sync with the GPU. Tearing is particularly visible in terminals because text is black-on-white (high contrast edges). Reject as default; expose as config option for users on variable-refresh displays.

**vs. `PresentMode::AutoNoVsync`**
Same as Immediate on most backends. Reject for same reason.

**vs. adaptive sync / `PresentMode::Mailbox`**
`Mailbox` (triple buffering) reduces latency vs FIFO but increases GPU memory usage (3 framebuffers) and power consumption. For a terminal emulator that does not need the lowest possible input latency (this is not a competitive game), the tradeoff is not worth it. Reject as default.

**vs. a custom frame pacer**
Implementing a frame pacer (e.g. waiting for the next vblank signal + scheduling work to arrive just-in-time) requires platform-specific APIs (`DwmFlush` on Windows, `VK_EXT_display_control` on Vulkan). Complex, fragile on WSL2, and provides marginal benefit for a terminal. Reject.

### WSL2-specific considerations

On WSL2, the GPU is accessed via WSLg (Windows Subsystem for Linux GUI). WSLg composites the Wayland surface from the Linux application into a Windows window using RDP. The effective vsync rate is determined by the Windows compositor (DWM), not the Linux application's present rate. Key implications:

- `wgpu` running inside WSL2 will target the WSLg-exposed DX12 or Vulkan adapter.
- `PresentMode::AutoVsync` will use whatever vsync mechanism WSLg exposes; in practice this is 60fps on most displays.
- If the Gunter binary runs as a native Windows executable (the preferred architecture: Windows binary spawns `wsl.exe` as the shell), vsync is handled directly by DWM with no WSLg layer. This is the recommended architecture: **Gunter as a native Windows binary, shell as WSL2 process**. This avoids all WSLg overhead.

### ConPTY window issues

The Windows binary architecture also avoids a WSLg-specific bug: WSLg's Wayland compositor does not correctly propagate ConPTY window resize events in all configurations. Running Gunter natively on Windows and using `portable-pty`'s ConPTY support directly means resize is handled by Windows' own ConPTY implementation, which is stable.

### Trade-offs accepted

- `AutoVsync` adds one frame of latency (worst case ~16ms at 60fps) compared to Immediate mode. Acceptable for a terminal; keyboard input is already buffered through the PTY layer.
- On displays that run at 144Hz or 240Hz, `FIFO` will still present at the display's native refresh rate, wasting GPU work on frames that look identical. Add a `max_fps` config option to cap the render rate independently of the display refresh rate.

---

## 6. WGSL Shader Design

### Decision

Use WGSL (WebGPU Shading Language). Two shader modules: one for background quads, one for glyph quads. No subpixel rendering (LCD ClearType). Linear-light blending in the fragment shader, output in sRGB.

### Vertex shader responsibilities

```wgsl
// Background pass vertex shader
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
    // Unit quad corners: (0,0), (1,0), (0,1), (1,1)
    let unit = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let pixel_pos = instance.pos + unit * instance.size;
    // Convert pixel coords to NDC: x in [-1,1], y in [-1,1] (y flipped)
    let ndc = vec2<f32>(
        (pixel_pos.x / viewport.width)  * 2.0 - 1.0,
        1.0 - (pixel_pos.y / viewport.height) * 2.0,
    );
    return VertexOutput(vec4<f32>(ndc, 0.0, 1.0), instance.bg);
}
```

The vertex shader does: unit quad expansion, pixel-to-NDC transformation, and passes color/UV to the fragment stage. No matrix multiplication needed — the NDC transformation is two FMAs per axis.

### Glyph pass fragment shader

```wgsl
@fragment
fn fs_glyph(in: GlyphVertexOutput) -> @location(0) vec4<f32> {
    let atlas_sample = textureSample(atlas, atlas_sampler, in.uv);
    // atlas_sample.a = coverage (0=transparent, 1=fully covered)
    // Premultiplied alpha blend: output = fg_color * coverage
    let color = in.fg * atlas_sample.a;
    return color;
}
```

For colored emoji glyphs (RGBA atlas samples), the blend mode changes: output the atlas sample directly, ignoring `fg`. This is detected at instance-build time by setting a flag in the instance data (or using a second draw call for emoji glyphs).

### Why not subpixel rendering (ClearType)

ClearType LCD rendering requires knowledge of the display's subpixel layout (RGB vs BGR vs vertical) and renders at 3× horizontal resolution, then filters. Problems:

1. On composited windows (all modern OSes), the terminal window background color bleeds through subpixel fringes, causing colored fringing unless the compositor supports gamma-correct compositing — which most do not, at the window boundary.
2. WSLg's RDP transport may downsample or compress the framebuffer, destroying subpixel information.
3. The implementation complexity is substantial.

Modern HiDPI displays (Retina, 4K) make subpixel rendering largely irrelevant because pixel density already exceeds perceptual limits. Use grayscale antialiasing only. For low-DPI displays, accept the slightly lower text quality.

### Why WGSL over GLSL/HLSL

- **WGSL is the native language for `wgpu`**. GLSL and HLSL require the `naga` transpiler (part of `wgpu`'s pipeline), which adds a compilation step and can produce less-optimal output on some backends.
- **GLSL** in `wgpu` requires the `wgpu::ShaderSource::Glsl` path, which has more caveats and less testing than the WGSL path.
- **HLSL** is Windows-only in spirit and would complicate any future cross-platform work.
- WGSL is designed to be unambiguous and safe — no undefined behavior at the language level, explicit numeric types. This catches shader bugs at compile time that GLSL allows silently.

### Trade-offs accepted

- WGSL is newer and less documentation-rich than GLSL. The `wgpu` examples and `naga` documentation fill this gap adequately.
- No subpixel rendering means slightly softer text on 96 DPI displays. Acceptable given the complexity cost.

---

## 7. Scrollback Rendering

### Decision

Scrollback buffer lives in `Grid::scrollback: VecDeque<Vec<Cell>>` (CPU memory only). When the user scrolls back, a `scroll_offset: usize` is set. The renderer computes which rows to display from `scroll_offset` to `scroll_offset + rows`, building `GlyphInstance`s from the appropriate rows. No scrollback data lives in GPU buffers.

### Why keep scrollback CPU-side

A 5000-line scrollback at 220 columns = 1,100,000 cells. At `GlyphInstance` size (40 bytes each) that would be 44 MB of GPU vertex buffer just for scrollback. This is wasteful because:

1. Only `rows` (e.g. 50) lines are visible at a time.
2. Scrollback is rarely accessed — most frames render the live viewport with zero scrollback offset.
3. Uploading 44 MB on every scroll event is worse than re-building 50 × 220 = 11,000 instances from CPU memory.

### Scrollback rendering flow

```
On scroll_offset change:
  1. Determine visible_rows = scrollback[scroll_offset .. scroll_offset + rows]
       (may mix scrollback lines and live grid rows if near the bottom)
  2. Build full Vec<GlyphInstance> for visible_rows (11,000 cells max)
  3. Upload entire instance buffer (full dirty)
  4. Draw normally

On each frame with scroll_offset == 0 (live view):
  1. Use per-cell dirty tracking as normal
  2. No scrollback data touched
```

The full upload on scroll events is 440 KB — a one-time cost when the user scrolls, not every frame. Once the user stops scrolling and the offset is stable, dirty tracking resumes for cursor blinks / incremental updates to the visible scrollback.

### Optimization: scrollback texture cache

If profiling shows scroll performance is poor (e.g. `cat /var/log/syslog` then rapid scroll), pre-rasterize the scrollback to a secondary texture (one row = one strip). This is a Phase 4+ optimization. Do not implement preemptively.

### Trade-offs accepted

- Full instance buffer upload on every scroll event (440 KB). Acceptable at 60fps where the user perceives no lag above ~100 MB/s upload rates, and GPU buffers are typically mapped persistently.
- `VecDeque<Vec<Cell>>` for scrollback means row access is O(1) but memory is fragmented (one heap allocation per row). For 5000 lines × 220 cells × ~12 bytes per Cell ≈ 13 MB. Acceptable. Consider a flat `Vec<Cell>` with row indexing if allocator pressure becomes measurable.

---

## 8. Color Spaces — sRGB Pipeline

### Decision

Store all colors as linear floating-point RGBA internally. The `GlyphInstance.fg` and `.bg` fields are linear RGBA `[f32; 4]`. Configure the `wgpu::Surface` with `TextureFormat::Bgra8UnormSrgb` (or `Rgba8UnormSrgb`). The GPU hardware performs the linear-to-sRGB gamma encoding at the final output stage automatically, with no manual gamma correction in shaders.

### Why linear-light blending

All color blending (alpha compositing of glyphs over backgrounds) must happen in linear light. If you blend in sRGB space, the result is perceptually wrong — edges appear too dark because sRGB gamma compresses dark values. Example: blending a gray glyph over a dark background in sRGB gives a different (incorrect) result than doing the same blend in linear light and converting to sRGB.

The correct pipeline is:
```
Config color (sRGB hex) → linear float → blend in shader → write to sRGB surface → display
```

The sRGB surface format means the GPU encodes the final linear value to sRGB on write, at no extra shader cost.

### Color input pipeline

```rust
// Convert Atom One Dark hex colors to linear RGBA at config load time
fn srgb_hex_to_linear(hex: u32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >>  8) & 0xFF) as f32 / 255.0;
    let b = ((hex >>  0) & 0xFF) as f32 / 255.0;
    // Apply sRGB to linear conversion (approximate: gamma 2.2, exact: piecewise)
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
```

This conversion runs once at startup (or on theme reload). All GPU operations see linear values.

### Why not gamma 2.2 approximation

The simple `c.powf(2.2)` approximation introduces error below 1% brightness values where sRGB has a linear segment. For terminal colors this is unlikely to matter visually, but the correct piecewise formula is not significantly more expensive and is more correct. Use the exact formula.

### Trade-offs accepted

- `Bgra8UnormSrgb` may not be supported on all adapters. Fall back to `Rgba8Unorm` + manual gamma in the fragment shader if `Bgra8UnormSrgb` is unavailable. Check adapter capability at init time via `adapter.get_texture_format_features`.
- Storing `fg`/`bg` as `[f32; 4]` costs 32 bytes per color pair per instance. Could pack as `u32` RGBA and convert in the vertex shader. Premature optimization; defer until profiling.

---

## 9. Background Quad Pass — Separate from Glyph Pass

### Decision

Two separate render passes:

1. **Pass 1 — background quads**: draw one solid-color rect per cell using `bg` color. No atlas texture sampling.
2. **Pass 2 — glyph quads**: draw one textured quad per non-space cell using the atlas texture and `fg` color, blended over whatever Pass 1 wrote.

Both passes use the same `GlyphInstance` buffer. The background pass uses only `pos`, `size`, and `bg`. The glyph pass uses all fields.

### Why separate passes over z-ordering in one pass

**Option A: Z-ordering, single pass, one draw call**
Render background quads and glyph quads in a single draw call using depth testing to ensure glyphs appear on top of backgrounds. Problems:
- Requires a depth buffer (extra memory, extra attachment).
- Alpha blending with depth testing is order-dependent and requires careful sorting. Transparent glyph anti-aliasing edges need the background color to already be present, which means front-to-back ordering for depth test does not work with blending.
- GPU depth testing adds fill-rate cost.

**Option B: Background color in the glyph shader, one pass**
Each glyph quad covers its full cell, draws the `bg` color first, then composites the glyph on top in the fragment shader. This is the approach some simpler terminal renderers use.
Problems:
- Cells with no glyph (space characters) still need to draw their background. You either draw a degenerate glyph quad (wasteful) or add a separate background-only draw call anyway.
- The fragment shader becomes more complex (two texture reads or a conditional).

**Option C: Two passes (chosen)**
Clean separation of concerns. The background pass is trivially simple (solid color rects, no texture). The glyph pass assumes the background is already present and only needs to composite the glyph alpha on top. This maps directly to how GPU blending works: `blend = src_alpha * src_color + (1 - src_alpha) * dst_color`. The `dst_color` at glyph draw time is already the correct background color from Pass 1.

No depth buffer needed. Blending is correct. Each shader is simple.

### Why not a stencil buffer

A stencil pass to mask glyph areas before drawing backgrounds would allow one combined pass with correct blending. This adds stencil attachment overhead and increases pipeline complexity. Not justified for a terminal's rendering complexity. Reject.

### Trade-offs accepted

- Two draw calls per frame instead of one. At GPU scale (one draw call = a few microseconds of CPU time), this is not a measurable cost.
- Background pass draws backgrounds for all cells including those whose glyph is a space — drawing pixels that will not be covered by a glyph. This is the correct behavior (cells must show their background color even if the character is a space).

---

## 10. WSL2 and Windows-Specific Considerations

### Decision

**Architecture**: Gunter runs as a native Windows (Win32) binary. The shell is spawned via `wsl.exe` through `portable-pty`'s ConPTY path. This is the same architecture as Windows Terminal.

**GPU backend**: prefer DX12 (`Backends::DX12`) on Windows. Fall back to Vulkan if DX12 init fails. Never use OpenGL on Windows.

### Why native Windows binary over WSL2 binary

| Concern | Native Windows binary | WSL2 binary (via WSLg) |
|---|---|---|
| GPU access | Direct DX12 via WDDMv3 | Paravirtualized via WSLg DX12 proxy |
| vsync | DWM direct | WSLg RDP compositor |
| ConPTY resize | Windows ConPTY API directly | WSLg may intercept and mangle |
| Font rendering | DirectWrite available | FreeType only |
| Binary size | Normal | Normal |
| Startup time | Fast | Needs WSLg compositor running |
| Display output | winit → HWND | winit → Wayland → WSLg → RDP → Windows |

The indirection in the WSLg path adds latency and fragility. Every layer (Wayland, RDP, WSLg compositor) is a potential source of tearing, resize glitches, or input lag. Running natively on Windows with `wsl.exe` as the shell captures all the benefit (WSL2 shell environment) without the display indirection.

### DX12 vs Vulkan on Windows

On modern Windows 10/11 with up-to-date GPU drivers, both DX12 and Vulkan are available. Prefer DX12 because:

1. `wgpu`'s DX12 backend is more mature and receives more testing than the Vulkan backend on Windows (historically, the DX12 backend has had fewer driver-specific bugs on Nvidia/AMD/Intel Windows drivers).
2. DX12 is the native API; the Vulkan driver on Windows is a translation layer (via the vendor's Vulkan ICD) that adds one more indirection.
3. `wgpu::Backends::DX12` works with WSL2's GPU paravirtualization (`dxgi1_2` and above are exposed through the GPU-PV layer).

Fall back to Vulkan (`Backends::VULKAN`) if DX12 device creation fails (older Windows 10, missing KB4015217 update, or running under a Hyper-V guest without GPU-PV).

### wgpu backend initialization

```rust
let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
    backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
    dx12_shader_compiler: wgpu::Dx12Compiler::Fxc,  // Fxc is always available; Dxc requires SDK
    ..Default::default()
});
```

Use `Dx12Compiler::Fxc` (the built-in compiler) rather than `Dxc` (requires DirectX Shader Compiler DLL) to avoid runtime dependency on `dxcompiler.dll`.

### ConPTY window resize

When the winit window is resized, the correct sequence is:
1. Recompute grid dimensions from new pixel size and cell size.
2. Call `pty.resize(PtySize { cols, rows, pixel_width, pixel_height })` on every active session.
3. Send `SIGWINCH` (handled automatically by `portable-pty` on Unix; ConPTY handles it on Windows).
4. Reflow the layout tree.
5. Set `all_dirty = true` on all grids.

The `pixel_width` and `pixel_height` fields of `PtySize` are used by ConPTY for DPI-aware rendering inside the PTY. Set them to the actual pixel dimensions of the pane rect, not zero.

### DPI scaling

On Windows with display scaling > 100%, winit reports physical pixels. The renderer operates in physical pixels throughout — no logical pixel abstraction. Cell size is computed as `font_size_px * dpi_scale`. When DPI changes (e.g. moving window between monitors), rebuild the glyph atlas at the new scale and set `all_dirty = true`.

### Trade-offs accepted

- Native Windows binary means the build system is a Windows toolchain. Cross-compilation from Linux for the Windows target is possible but requires the MSVC target or MinGW. Developers on Linux use WSL to build or use a Windows dev machine.
- `Dx12Compiler::Fxc` produces slightly less optimized shaders than `Dxc`. For the simple shaders in `gunter-renderer`, this is undetectable.
- Falling back to Vulkan on failure adds ~50 lines of init code. Worth the compatibility gain.

---

## Appendix: Decision Matrix

| Decision | Chosen | Key reason to reject alternatives |
|---|---|---|
| GPU backend | `wgpu` | Safety, portability, WSL2 DX12 support |
| Font rasterizer | `wezterm-font` | Nerd Font, shaping, fallback — already solved |
| Draw strategy | Instanced draw | One draw call vs 11,000 |
| Glyph shader | Grayscale AA, no ClearType | Compositing incompatibility, HiDPI irrelevance |
| Shader language | WGSL | Native to `wgpu`, no transpiler risk |
| Dirty tracking | Per-cell `Vec<bool>` | Cursor blink = 1 cell; per-row wastes 220x |
| Frame timing | `AutoVsync` (FIFO) | No tearing, sufficient for terminal latency |
| Scrollback GPU | CPU-only scrollback | 44 MB GPU buffer for 5000-line scrollback is wasteful |
| Color space | Linear internal, sRGB surface | Correct blending on dark backgrounds |
| Render passes | Two passes (bg then glyph) | Clean blending without depth buffer |
| Platform arch | Native Windows binary | Avoids WSLg RDP/Wayland indirection |
| DX12 vs Vulkan | DX12 primary | Native Windows API, fewer driver bugs |
