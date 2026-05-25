# Gunter — Rust Terminal Emulator

> Fast, GPU-accelerated terminal for Windows/WSL. Inspired by kitty and tmux.
> Atom One Dark theme · ComicShannsMono Nerd Font Mono

---

## Design Goals

- **Speed first** — GPU rendering via `wgpu`, zero DOM, no Electron
- **Low resource** — lean binary (~15 MB), minimal RAM per pane
- **Multi-shell** — WSL, PowerShell, CMD, any POSIX shell
- **Multiplexed** — tmux-style pane splits, tabs, optional socket server
- **Embeddable** — named pipe / Unix socket so other tools can attach

---

## Crate Stack

| Crate            | Role                                                |
| ---------------- | --------------------------------------------------- |
| `wgpu`           | GPU rendering (DX12/Vulkan/Metal — one codebase)    |
| `winit`          | Cross-platform window + OS event loop               |
| `vte`            | Zero-alloc VT/ANSI escape code parser               |
| `portable-pty`   | PTY allocation on Windows, macOS, Linux             |
| `tokio`          | Async runtime, `mpsc` channels between layers       |
| `wezterm-font`   | Glyph rasterization + atlas (vendored from WezTerm) |
| `arboard`        | Clipboard read/write (copy/paste)                   |
| `serde` + `toml` | Config file parsing                                 |
| `notify`         | Config hot-reload via filesystem watch              |
| `uuid`           | Session IDs                                         |
| `clap`           | CLI argument parsing (`gunter attach <id>`)         |

---

## Workspace Layout

```
gunter/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── gunter-core/            # PTY management, session model, config types
│   ├── gunter-renderer/        # wgpu surface, glyph atlas, grid draw calls
│   ├── gunter-term/            # vte parser → terminal grid state
│   ├── gunter-input/           # keyboard/mouse → action mapping
│   └── gunter-app/             # main binary — winit loop, wires all crates
├── config/
│   └── default.toml
└── assets/
    └── themes/
        └── atom-one-dark.toml
```

---

## Default Theme — Atom One Dark

```toml
# assets/themes/atom-one-dark.toml

[colors]
background  = "#282c34"
foreground  = "#abb2bf"
cursor      = "#528bff"
selection   = "#3e4451"

black        = "#3f4451"
red          = "#e06c75"
green        = "#98c379"
yellow       = "#e5c07b"
blue         = "#61afef"
magenta      = "#c678dd"
cyan         = "#56b6c2"
white        = "#abb2bf"

bright_black   = "#4f5666"
bright_red     = "#e06c75"
bright_green   = "#98c379"
bright_yellow  = "#e5c07b"
bright_blue    = "#61afef"
bright_magenta = "#c678dd"
bright_cyan    = "#56b6c2"
bright_white   = "#ffffff"
```

---

## Default Config

```toml
# ~/.config/gunter/config.toml

[shell]
program = "wsl.exe"
args    = []
# For a specific distro:
# args = ["--distribution", "Ubuntu-22.04"]

[font]
family      = "ComicShannsMono Nerd Font Mono"
fallback    = []
size        = 14.0
line_height = 1.2

[theme]
name = "atom-one-dark"

[scrollback]
lines = 5000

[keys]
new_tab    = "Ctrl+Shift+T"
close_tab  = "Ctrl+Shift+W"
split_h    = "Ctrl+Shift+H"
split_v    = "Ctrl+Shift+V"
next_pane  = "Ctrl+Tab"
prev_pane  = "Ctrl+Shift+Tab"
zoom_pane  = "Ctrl+Z"
```

---

## Core Data Types

### `gunter-core/src/grid.rs`

```rust
#[derive(Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,   // bold | italic | underline | blink | inverse
}

pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub cursor: (u16, u16),
    pub dirty: Vec<bool>,        // per-cell dirty flag — set on write, cleared after render; serves as the frame diff
    pub scroll_top: u16,         // DECSTBM top margin (0-indexed)
    pub scroll_bot: u16,         // DECSTBM bottom margin (inclusive)
    pub cursor_visible: bool,
}

// Required methods:
// fn scroll_up(&mut self, n: u16)   — push rows to scrollback, shift up within scroll region
// fn scroll_down(&mut self, n: u16) — shift down within scroll region
```

### `gunter-core/src/session.rs`

```rust
pub struct Session {
    pub id:          Uuid,
    pub pty:         Box<dyn MasterPty + Send>,
    pub grid:        Grid,
    pub alt_grid:    Grid,    // alternate screen — swapped on CSI ?1049h/l
    pub parser:      vte::Parser,
    pub tx:          mpsc::Sender<Vec<u8>>,   // keystrokes → PTY stdin
    pub rect:        Rect,                    // pixel rect in window
    pub alt_active:  bool,
}
```

### `gunter-core/src/layout.rs`

```rust
// Binary split tree — same model as i3 / tmux
pub enum Layout {
    Leaf(Uuid),                                    // session id
    Split { axis: Axis, ratio: f32, left: Box<Layout>, right: Box<Layout> },
}

pub enum Axis { Horizontal, Vertical }
```

---

## Build Phases

### Phase 1 — PTY + VTE grid ✅ Done

**Goal:** type a command in WSL and see ASCII output.

1. `cargo new --workspace gunter`
2. Create `gunter-core`, `gunter-term`, `gunter-app` crates
3. Open a PTY with `portable_pty::native_pty_system()`
4. Spawn `wsl.exe` into the PTY slave
5. Read PTY stdout in a tokio task → push bytes to `vte::Parser`
6. `vte::Perform` impl writes into `Grid`
7. Print grid to stdout (no window yet) to verify ANSI parsing

```rust
// gunter-app/src/main.rs  — Phase 1 skeleton
// No async needed yet — tokio is added in Phase 2 when channels appear
fn main() {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize { rows: 24, cols: 80, ..Default::default() }).unwrap();

    let cmd = CommandBuilder::new("wsl.exe");
    let _child = pair.slave.spawn_command(cmd).unwrap();

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut parser = vte::Parser::new();
    let mut grid   = Grid::new(80, 24);
    let mut performer = GridPerformer { grid: &mut grid };

    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for byte in &buf[..n] {
                    parser.advance(&mut performer, *byte);
                }
            }
        }
    }
}
```

**Exit condition:** `echo hello` in WSL prints "hello" in the grid buffer. ✅

---

### Phase 2 — Interactive Core

**Goal:** user can type in shell, prompt renders correctly, vim opens without corruption.

#### Keyboard → PTY write

Handle `WindowEvent::KeyboardInput` (winit) and write to `pair.master.try_clone_writer()`.

Key translation table:

| Key | Bytes written |
|-----|---------------|
| Printable chars | UTF-8 bytes |
| Enter | `\r` |
| Backspace | `\x7f` |
| Tab | `\x09` |
| Ctrl+A..Z | `\x01`..\x1a` |
| Arrow Up/Down/Left/Right | `\x1b[A` / `\x1b[B` / `\x1b[C` / `\x1b[D` |
| Ctrl+Arrow Up/Down/Left/Right | `\x1b[1;5A` / `\x1b[1;5B` / `\x1b[1;5C` / `\x1b[1;5D` |
| F1–F12 | standard xterm sequences |
| Home, End, PgUp, PgDn, Delete | standard xterm sequences |

#### Required VTE sequences — minimum for shell usability

Add to `GridPerformer` CSI dispatch:

| Sequence | Action |
|----------|--------|
| `CSI J` / `0J` | Erase cursor→end of display |
| `CSI 1J` | Erase start→cursor |
| `CSI 2J` / `3J` | Erase entire display (clear screen) |
| `CSI K` / `0K` | Erase cursor→end of line |
| `CSI 1K` | Erase start of line→cursor |
| `CSI 2K` | Erase entire line |
| `CSI r` (DECSTBM) | Set scroll region top/bottom → `grid.scroll_top` / `grid.scroll_bot` |
| `CSI S` | Scroll up N lines |
| `CSI T` | Scroll down N lines |
| `CSI L` | Insert N lines |
| `CSI M` | Delete N lines |
| `CSI @` | Insert N chars |
| `CSI P` | Delete N chars (DCH) |
| `CSI X` | Erase N chars (ECH) |
| `CSI ?1049h` / `CSI ?1049l` | Enter / exit alternate screen |
| `CSI ?25h` / `CSI ?25l` | Show / hide cursor → `grid.cursor_visible` |
| `CSI ?2004h` / `CSI ?2004l` | Enable / disable bracketed paste |

#### PTY resize

On `WindowEvent::Resized`: compute new `(cols, rows)` from `pixel_size / cell_size`, then call `pair.master.resize(PtySize { cols, rows, .. })`.

**Exit condition:** `bash`/`fish`/`zsh` prompt renders without garbage, `vim` opens and renders correctly.

---

### Phase 3 — GPU Renderer + Font

**Goal:** render the grid at 60fps, `ComicShannsMono Nerd Font Mono`, Atom One Dark colors, cursor visible.

Pipeline:

```
Grid (CPU)
  → diff against prev frame → Vec<GlyphInstance { x, y, uv, fg, bg }>
  → upload to wgpu vertex buffer (only dirty cells)
  → instanced draw call: glyph atlas texture + vertex buffer → wgpu::Surface
```

Steps:

1. Init `winit::Window` + `wgpu::Instance` → `wgpu::Surface`
2. Build glyph atlas: rasterize each char once with `wezterm-font`, pack into a `wgpu::Texture`
3. Store UV rect per glyph in a `HashMap<(char, CellFlags), UvRect>`
4. Each frame: walk dirty cells only, build `Vec<GlyphInstance>`, upload, draw; clear dirty after render
5. Two render passes: background quads first (colored rects), then glyph quads on top

Glyph instance layout for WGSL shader:

```rust
#[repr(C)]
pub struct GlyphInstance {
    pub pos:    [f32; 2],   // top-left pixel position
    pub size:   [f32; 2],   // cell size in pixels
    pub uv_pos: [f32; 2],   // atlas UV top-left
    pub uv_sz:  [f32; 2],   // atlas UV size
    pub fg:     [f32; 4],   // RGBA
    pub bg:     [f32; 4],
}
```

Additional pieces required:

- **`wezterm-font` instead of `fontdue`** — proper Nerd Font glyph support, font fallback chain, correct metrics
- **Truecolor SGR** — `38;2;r;g;b` / `48;2;r;g;b` in performer; `Cell.fg/bg: Color` already exists, add SGR parsing
- **256-color SGR** — `38;5;n` / `48;5;n` — map via xterm-256 palette
- **Bright color SGR** — `90–97` fg, `100–107` bg
- **Cursor rendering** — extra quad at `grid.cursor` position, color from theme `cursor`, style from `DECSCUSR` (block/bar/underline)
- **Dirty-only upload** — skip instances where `!grid.dirty[i]`; clear dirty after render

**Exit condition:** WSL prompt renders in ComicShannsMono with correct Atom One Dark colors, Nerd Font icons visible, cursor visible as block.

---

### Phase 4 — Input Complete + Clipboard

**Goal:** copy/paste works, mouse scrolls history, modifier keys work in vim.

- **Full modifier keyboard:** Ctrl+arrows → `\x1b[1;5A` etc., Alt+key → `\x1b` + key byte, Shift+F-keys
- **Mouse scroll wheel** → scroll through scrollback buffer (no PTY write needed, just offset render)
- **Text selection:** mouse drag → mark cell range, `Ctrl+Shift+C` → copy to clipboard via `arboard` crate
- **Paste:** `Ctrl+Shift+V` → get clipboard string → if `?2004h` active wrap in `\x1b[200~...\x1b[201~`, write to PTY
- **Mouse reporting:** when shell enables `?1000h`/`?1006h`, encode mouse events as `\x1b[<btn;col;rowM/m` and write to PTY (needed for vim mouse, fzf)
- **Scrollback render:** when scroll offset > 0, render from `scrollback` buffer not live grid

**Exit condition:** can copy text with mouse + `Ctrl+Shift+C`, paste with `Ctrl+Shift+V`, vim mouse works, scrollback navigable.

---

### Phase 5 — Multiplexing

**Goal:** `Ctrl+Shift+H` / `Ctrl+Shift+V` splits panes; `Ctrl+Shift+T` opens a new tab.

1. Add `Layout` tree to window state
2. On split: insert a `Layout::Split` node, divide parent rect by `ratio`
3. On resize: walk the tree, recompute `Rect` for every `Leaf`, call `pty.resize(cols, rows)` on each session
4. On `Ctrl+L` / `Ctrl+H`: walk the tree to find the adjacent pane in that direction
5. Tabs = `Vec<Layout>` — one layout tree per tab, only the active tab renders

Resize propagation:

```rust
fn reflow(layout: &Layout, rect: Rect, sessions: &mut HashMap<Uuid, Session>) {
    match layout {
        Layout::Leaf(id) => {
            let s = sessions.get_mut(id).unwrap();
            s.rect = rect;
            s.pty.resize(PtySize { cols: rect.cols(), rows: rect.rows(), .. }).ok();
        }
        Layout::Split { axis, ratio, left, right } => {
            let (lr, rr) = rect.split(*axis, *ratio);
            reflow(left,  lr, sessions);
            reflow(right, rr, sessions);
        }
    }
}
```

**Exit condition:** two WSL panes side by side, independent input/output, correct resize.

---

### Phase 6 — Config + Hot Reload

**Goal:** all behaviour driven by `~/.config/gunter/config.toml`, changes apply without restart.

1. Load config at startup with `serde` + `toml`
2. Watch config file with `notify` crate
3. On change event: re-parse, diff against current config
4. Apply only the changed fields (theme swap reloads color uniforms in wgpu; font change rebuilds atlas)

Font fallback chain (for Nerd Font glyphs + CJK):

```toml
[font]
family   = "ComicShannsMono Nerd Font Mono"
fallback = ["Noto Sans CJK", "Segoe UI Emoji"]
size     = 14.0
```

**Exit condition:** change `size = 18.0` in config, Gunter redraws at new size without restart.

---

### Phase 7 — Socket Server (stretch)

**Goal:** open a pane with PowerShell or CMD; let external tools (Neovim, VS Code) attach to a Gunter session.

Multi-shell: `CommandBuilder` is already shell-agnostic — pass `pwsh.exe` or `cmd.exe` instead of `wsl.exe`. Surface a shell selector in config (`[shell] program`) and in the new-tab menu.

Socket server (optional daemon mode):

```rust
// gunter-core/src/server.rs
// Windows: \\.\pipe\gunter-{session-id}
// WSL/Linux: /tmp/gunter-{session-id}.sock
//
// Protocol: raw byte stream (same as a PTY) — stdin in, stdout out.
// No framing needed; each connection maps to one session.
```

This means Neovim running inside WSL can call `gunter attach <id>` and get a full PTY. Matches tmux's attach model.

**Exit condition:** `gunter attach` from a second terminal window joins the existing session.

---

## Phase Summary

| Phase | Deliverable | Key crates | Complexity | Status |
|-------|-------------|------------|------------|--------|
| 1 | PTY + VTE grid | `portable-pty`, `vte` | Low | ✅ Done |
| 2 | Interactive core — keyboard + critical VTE + PTY resize | internal | Medium | 🔲 |
| 3 | GPU renderer — wgpu + wezterm-font + cursor + colors | `wgpu`, `winit`, `wezterm-font` | High | 🔲 (partial) |
| 4 | Input complete — mouse, clipboard, scrollback nav | `arboard` | Medium | 🔲 |
| 5 | Multiplexing — pane splits, tabs | internal | Medium | 🔲 |
| 6 | Config + hot reload | `serde`, `toml`, `notify` | Low | 🔲 |
| 7 | Socket server (stretch) | `tokio::net` | Medium | 🔲 |

---

## Implementation Notes

- **Complete Phase 2 before Phase 3** — the renderer draws whatever the grid contains; if the grid is wrong due to missing VTE sequences, the GPU just renders the wrong content faster.
- **Start with Phase 1 before touching the GPU.** The PTY → vte → Grid pipeline is the foundation. If cell state is wrong, the renderer just draws it wrong faster.
- **Dirty tracking is critical for performance.** Only upload changed cells to the GPU vertex buffer. A full 220×50 grid is 11,000 cells — uploading all of them every frame is wasteful even with instancing.
- **`wezterm-font` is the pragmatic choice** for the atlas. Writing a font rasterizer from scratch (FreeType bindings or `fontdue`) is a separate multi-week project. Vendoring the relevant parts of WezTerm's font layer saves that time.
- **`portable-pty` handles the Windows/WSL PTY complexity.** On Windows, `wsl.exe` communicates via a ConPTY handle — `portable-pty` abstracts this correctly.
- **Nerd Font glyphs** (icons, powerline symbols) are in the private use area (U+E000–U+F8FF). The glyph atlas must handle them — `wezterm-font` does this natively.

---

## References

- [portable-pty docs](https://docs.rs/portable-pty)
- [vte docs](https://docs.rs/vte)
- [wgpu examples](https://github.com/gfx-rs/wgpu/tree/trunk/examples)
- [WezTerm source](https://github.com/wez/wezterm) — reference for font + renderer
- [Alacritty source](https://github.com/alacritty/alacritty) — reference for grid + vte integration
- [ComicShannsMono](https://github.com/shannpersand/comic-shanns) — font source
