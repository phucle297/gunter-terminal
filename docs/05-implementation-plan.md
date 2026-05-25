# Gunter — Implementation Plan

> Versioned delivery plan for the Gunter Rust terminal emulator.
> Each version is 1–3 weeks of focused work with a concrete, demo-able outcome.
> Versions map directly to SPEC.md build phases but split them into independently shippable increments.

---

## Version Index

| Version | Name | Key deliverable | Complexity |
|---|---|---|---|
| v0.1.0 | Headless PTY Shell | `cargo run` → WSL prompt in stdout, VTE parsing verified | Low |
| v0.2.0 | GPU Window | Native window, Atom One Dark, ComicShannsMono, 60fps | High |
| v0.3.0 | Keyboard + Interaction | Fully interactive single-pane terminal | High |
| v0.4.0 | Splits and Tabs | tmux-style multiplexer milestone | Medium |
| v0.5.0 | Multi-Shell + Socket Server | Any shell, `gunter attach` works | Medium |
| v0.6.0 | Config and Hot-Reload | Full TOML config, live reload, theme system | Low |
| v1.0.0 | Production Hardening | Perf targets met, copy/paste, mouse, packaging | — |

---

## v0.1.0 — Headless PTY Shell

**Goal:** Run `cargo run` and see a live WSL shell prompt printed to stdout — proving that PTY allocation, VTE parsing, and Grid state are all correct before any GPU work begins.

**Exit criteria:**
- `cargo run` opens a PTY, spawns `wsl.exe`, and prints the shell prompt to stdout using a naive grid-dump function.
- Typing `echo hello` in the spawned shell results in `hello` appearing in the grid buffer (verified via stdout dump or a test harness).
- The VTE parser handles ANSI color escape codes: cell `fg`/`bg` fields are set correctly on colored prompt output.
- The PTY reader loop runs on a tokio task; the main thread is not blocked.
- All three foundational crates (`gunter-core`, `gunter-term`, `gunter-app`) exist with their Cargo.toml entries in the workspace.
- `cargo test` passes a unit test that feeds a known ANSI byte sequence into the VTE performer and asserts the resulting grid cell values.

**Crates modified:** `gunter-core`, `gunter-term`, `gunter-app`

**Key technical work:**

- **Workspace scaffold.** Create the Cargo workspace root and five crate skeletons (`gunter-core`, `gunter-renderer`, `gunter-term`, `gunter-input`, `gunter-app`). Only `gunter-core`, `gunter-term`, and `gunter-app` get real code in this version. `gunter-renderer` and `gunter-input` are stubs with an empty `lib.rs`.

- **Grid type (`gunter-core/src/grid.rs`).** Implement `Cell`, `CellFlags`, `Color`, and `Grid` exactly as specified in SPEC.md. `Grid::new(cols, rows)` allocates `cells`, `dirty`, and `scrollback`. Implement `Grid::dump_to_stdout()` for the headless debug output. The `dirty` field is a `Vec<bool>` (one per cell) as decided in `docs/02-renderer.md` §4.

- **Session type (`gunter-core/src/session.rs`).** Implement `Session` with `id: Uuid`, `pty: Box<dyn MasterPty + Send>`, `grid: Grid`, `parser: vte::Parser`, `tx: mpsc::Sender<Vec<u8>>`, and `rect: Rect`. `Session::spawn(shell, cols, rows)` calls `portable_pty::native_pty_system().openpty()` and `CommandBuilder::new(shell)` — this is the exact pattern from SPEC.md Phase 1. The tokio reader task is started inside `spawn`.

- **VTE performer (`gunter-term/src/performer.rs`).** Implement `vte::Perform` for `GridPerformer<'_>`. Handle at minimum: `print` (character output → set cell, mark dirty), `execute` (C0 controls: `\r`, `\n`, `\x08` backspace, `\x07` bell noop), `csi_dispatch` for SGR (color and attribute sequences), cursor movement (CUP, CUF, CUB, CUU, CUD), and erase commands (ED, EL). These cover 95% of prompt output. Reference Alacritty's `term/src/ansi.rs` for the dispatch table.

- **Tokio PTY reader loop (`gunter-app/src/main.rs`).** Spawn a `tokio::task` that reads PTY stdout in a `[u8; 4096]` buffer loop, pushes each byte through `vte::Parser::advance`, and calls `Grid::dump_to_stdout()` after each buffer read. The `tx: mpsc::Sender<Vec<u8>>` channel is used to write keystrokes back to the PTY stdin — wired up but not exercised until v0.3.0.

- **Unit test harness (`gunter-term/tests/vte_parser.rs`).** Feed `b"\x1b[32mhello\x1b[0m"` into the performer and assert `grid.cells[0].ch == 'h'` and `grid.cells[0].fg == Color::Green`. This test is the regression anchor for all VTE work.

**Depends on:** Nothing. This is the foundation.

**Risks / open questions:**
- `portable-pty` on Windows requires the MSVC toolchain and ConPTY (Windows 10 1809+). If the dev machine is older, `openpty` will panic. Document the minimum Windows version.
- `wsl.exe` must be in `PATH`. On a machine without WSL2 installed, the spawn will fail. Add a fallback to `cmd.exe` or a configurable shell for the test runner.
- `vte` crate version: Alacritty pins to a specific minor version. Confirm the version used matches the API expected (especially `Perform::hook` vs `Perform::osc_dispatch` signatures differ between 0.10 and 0.13).

**Why this version boundary:** The PTY → VTE → Grid pipeline is fully exercised and testable with zero GPU dependency. A broken grid draws wrong at 60fps — this checkpoint proves correctness before complexity multiplies. The stdout dump is demoable to anyone via SSH.

---

## v0.2.0 — Native Window with GPU Text Rendering

**Goal:** `cargo run` opens a native OS window rendering the WSL shell prompt in ComicShannsMono Nerd Font Mono with Atom One Dark colors at 60fps — the terminal is visible but not yet interactive.

**Exit criteria:**
- A native window opens via `winit` on Windows (DX12 backend preferred, Vulkan fallback).
- The WSL prompt and any PTY output render in the correct Atom One Dark colors (`#282c34` background, `#abb2bf` foreground).
- ComicShannsMono Nerd Font Mono glyphs render correctly, including Nerd Font private-use-area codepoints (test: a prompt containing a git branch icon).
- The frame rate is stable at 60fps as measured by a frame timer logged to stderr; no frame exceeds 33ms (2× budget).
- Only dirty cells generate vertex buffer uploads; a cursor blink (one dirty cell) should generate exactly one 40-byte write (verified by a renderer stats counter).
- Resizing the window recomputes cell dimensions, rebuilds the glyph atlas at the new DPI, and redraws correctly.
- `gunter-renderer` is a real crate with a public `Renderer` struct that `gunter-app` instantiates.

**Crates modified:** `gunter-renderer` (major new work), `gunter-app` (add winit event loop)

**Key technical work:**

- **wgpu surface init (`gunter-renderer/src/surface.rs`).** Initialize `wgpu::Instance` with `Backends::DX12 | Backends::VULKAN` using `Dx12Compiler::Fxc` (no DLL dependency) as specified in `docs/02-renderer.md` §10. Create `wgpu::Surface` from the winit `WindowHandle`. Select the surface format: prefer `Bgra8UnormSrgb`; fall back to `Rgba8Unorm` with manual gamma in the fragment shader if unsupported. The `Renderer::new(window, grid_ref)` constructor performs all of this.

- **Glyph atlas (`gunter-renderer/src/atlas.rs`).** Vendor or depend on `wezterm-font` for rasterization. Build a `GlyphAtlas` backed by a `wgpu::Texture` (RGBA8, 2048×2048 initial size). Use a shelf/row allocator as described in `docs/02-renderer.md` §2. Warmup at startup: rasterize printable ASCII (U+0020–U+007E) for all combinations of `CellFlags::BOLD | ITALIC` at the configured font size. Store `GlyphKey → UvRect` in a `HashMap`. Lazy-fill on first encounter of out-of-warmup glyphs (Nerd Font icons, CJK). Note: vendoring `wezterm-font` requires pulling its `config`, `rangeset`, and `font-loader` subcrates. Allocate dedicated time for this — it is the highest-risk item in this version.

- **Instanced draw pipeline (`gunter-renderer/src/pipeline.rs`).** Implement the two-pass rendering design from `docs/02-renderer.md` §3 and §9. Pass 1 renders background quads (solid color rects, no atlas sample). Pass 2 renders glyph quads (atlas texture, premultiplied alpha blend over Pass 1 output). Both passes use the same `GlyphInstance` layout (40 bytes per instance as specified in SPEC.md). The vertex buffer holds one instance per cell; only dirty ranges are written via `queue.write_buffer`. Two WGSL shader modules: `bg.wgsl` and `glyph.wgsl`. Color values are stored as linear RGBA `[f32; 4]`; the sRGB surface handles gamma encoding on output (see `docs/02-renderer.md` §8).

- **Dirty-tracking integration (`gunter-renderer/src/frame.rs`).** Each render frame: read `Grid::dirty` bitvec, build `GlyphInstance` for each dirty cell, compute contiguous dirty ranges, call `queue.write_buffer` per range, clear dirty flags. `Grid::all_dirty = true` triggers a full upload (used on resize, theme change). The `Grid` is behind a `RwLock<Grid>`; the renderer acquires a read lock for the frame build, the VTE performer acquires a write lock for cell updates.

- **winit event loop (`gunter-app/src/app.rs`).** Replace the Phase 1 stdout main with a `winit::event_loop::EventLoop` using `ApplicationHandler`. On `Resumed`: create window + `Renderer`. On `RedrawRequested`: call `renderer.render()`. On `Resized`: call `renderer.resize(new_size)` which recomputes cell dimensions and sets `all_dirty = true`. The PTY reader tokio task (from v0.1.0) continues running; it sends a `RequestRedraw` event via `event_loop_proxy` whenever the grid is mutated.

- **sRGB color pipeline (`gunter-renderer/src/color.rs`).** Implement `srgb_hex_to_linear(hex: u32) -> [f32; 4]` using the exact piecewise formula from `docs/02-renderer.md` §8. Convert Atom One Dark hex values at config load time (v0.6.0 will wire this to the TOML config; for now hardcode Atom One Dark). Store `Theme` as a struct of `[f32; 4]` color arrays — this struct becomes the wgpu uniform buffer in the fragment shader.

**Depends on:** v0.1.0 PTY + grid pipeline. The renderer reads from `Grid` but does not own it. `RwLock<Grid>` shared between the PTY tokio task (writer) and the winit render loop (reader).

**Risks / open questions:**
- `wezterm-font` vendoring is the highest-risk item in the project. It was not designed as a standalone crate. Plan for 3–5 days of build system work. If vendoring proves impossible in the v0.2.0 timeframe, fall back to `fontdue` for ASCII-only rendering and defer full Nerd Font support to v0.6.0. Document this fallback in a TODO.
- DPI scaling on Windows: winit reports physical pixels. Confirm that the `ScaleFactorChanged` event triggers a proper atlas rebuild at the new Dpi scale.
- `wgpu` 0.20+ changed the `Surface::get_current_texture` API. Pin the version and document it.
- WSL2-specific: Gunter is a native Windows binary; confirm `wgpu` DX12 surface creation works in a Windows toolchain build (`x86_64-pc-windows-msvc`). Cross-compilation from a Linux host requires the MSVC target — document this constraint.

**Why this version boundary:** This is the most complex version in the project. Completing it proves the GPU renderer is working and the glyph atlas is correct. Everything after this version builds on a working visual foundation. The exit criteria are directly demoable: open the terminal, see text.

---

## v0.3.0 — Keyboard Input and Basic Interaction

**Goal:** The terminal is fully interactive: keystrokes reach the shell, the output renders, and the window responds correctly to resize — Gunter is usable as a single-pane daily driver.

**Exit criteria:**
- Typing any key sends the correct byte(s) to the PTY stdin; `echo`, tab completion, and Ctrl+C all work.
- `Ctrl+C` sends SIGINT (byte `0x03`) to the PTY and does not trigger any Gunter-level action.
- Mouse click in the window positions the cursor (by sending the appropriate VT mouse event to the PTY) when the shell supports it; focus-click works even when mouse reporting is off.
- Window resize correctly calls `pty.resize(PtySize { cols, rows, pixel_width, pixel_height })` on the active session with the actual pixel dimensions as specified in `docs/02-renderer.md` §10.
- `Ctrl+Shift+V` pastes clipboard contents wrapped in bracketed paste sequences (`\e[200~...\e[201~`) as specified in `docs/04-ux-config.md` §11.
- The `gunter-input` crate is real: it exports `KeyCombo`, `Action` enum, and `InputMap::lookup(combo) -> Option<Action>`.
- Running `vtebench` or `cat /dev/urandom | head -c 10M` does not crash or freeze the window.

**Crates modified:** `gunter-input` (new real implementation), `gunter-core` (PTY stdin write path), `gunter-app` (keyboard event dispatch), `gunter-term` (bracketed paste, mouse reporting mode tracking)

**Key technical work:**

- **Action enum and InputMap (`gunter-input/src/lib.rs`).** Define the `Action` enum as specified in `docs/04-ux-config.md` §3 (NewTab, CloseTab, SplitHorizontal, SplitVertical, FocusNextPane, FocusPrevPane, ZoomPane, ScrollUp, ScrollDown, CopySelection, PasteClipboard, and SendToPane as the passthrough variant). Implement `KeyCombo { modifiers: Modifiers, key: Key }` with `Hash + Eq`. Implement `InputMap` backed by a `HashMap<KeyCombo, Action>`. Hardcode the default bindings from `docs/04-ux-config.md` §4 for now (config wiring comes in v0.6.0). `InputMap::lookup` returns `None` for keys that should pass through to the PTY.

- **Keyboard event dispatch (`gunter-app/src/input.rs`).** In the winit `WindowEvent::KeyboardInput` handler: call `input_map.lookup(combo)`. If `Some(action)`, dispatch to the appropriate handler (most are no-ops until v0.4.0). If `None`, encode the key as bytes using a key-to-bytes function and write to `session.tx`. Handle modifier combinations: `Ctrl+key` → byte `0x01..0x1a`, `Alt+key` → `\x1b` prefix, function keys → VT sequences. Reference Alacritty's `src/input.rs` key encoding table.

- **PTY stdin write path (`gunter-core/src/session.rs`).** Implement the `tx` channel consumer: a tokio task that receives `Vec<u8>` from `session.tx` and calls `pty.master.write_all(&bytes)`. This closes the loop: winit key event → `session.tx.send(bytes)` → PTY stdin write.

- **Mouse click handling (`gunter-app/src/input.rs`).** On `WindowEvent::MouseInput` with left button: determine which pane the click landed in (trivial with a single pane). If the pane is already focused, and the PTY has not requested mouse events, forward the click as a cursor positioning attempt. Implement the click-to-focus semantics from `docs/04-ux-config.md` §12.

- **Mouse reporting mode tracking (`gunter-term/src/modes.rs`).** The VTE performer must track when an application sends `\e[?1000h` (X10 mouse enable) or `\e[?1000l` (disable). Store a `MouseReportingMode` enum on `Session`. The input handler checks this before deciding whether to forward or handle mouse events. In forwarding mode, Shift+click still triggers Gunter-level selection as specified in `docs/04-ux-config.md` §12.

- **Bracketed paste (`gunter-term/src/paste.rs`).** Implement `Session::paste(text: &str)` which wraps the text in `\x1b[200~{text}\x1b[201~` and sends it through `session.tx`. Called from the Ctrl+Shift+V handler. Bracketed paste mode is enabled at PTY startup by sending `\x1b[?2004h` immediately after spawn.

- **Resize correctness (`gunter-app/src/app.rs`).** On `WindowEvent::Resized`: compute new `cols = floor(pixel_width / cell_width)` and `rows = floor(pixel_height / cell_height)`. Call `session.pty.resize(PtySize { cols, rows, pixel_width, pixel_height })`. Call `grid.resize(cols, rows)` (implement this method in `gunter-core` — truncate or pad cells, reset dirty). Set `all_dirty = true`.

**Depends on:** v0.2.0 renderer (window exists, PTY running, grid rendering). This version adds the reverse path: input → PTY.

**Risks / open questions:**
- Key encoding edge cases: `Ctrl+Space`, dead keys, IME input on Windows. `winit` exposes `KeyEvent::text` for printable characters — use that for the passthrough path to avoid reimplementing key-to-char mapping.
- `vtebench` throughput test: if the PTY reader loop cannot keep up with high-volume output, the `RwLock<Grid>` write lock will contend with the render loop's read lock. If this appears, switch to a double-buffer scheme (VTE task writes to a back buffer; render task swaps the front buffer once per frame).
- Windows clipboard access for paste (`arboard` crate or `winit`'s clipboard API) may block the event loop thread briefly on large paste buffers. Use `tokio::spawn_blocking` for clipboard reads > 64 KB.

**Why this version boundary:** After this version Gunter is a usable terminal for daily work. All further work is multiplexer features, polish, and performance — none of it is required for basic operation. This is a natural dogfooding checkpoint.

---

## v0.4.0 — Pane Splits and Tabs

**Goal:** `Ctrl+Shift+H`, `Ctrl+Shift+V`, `Ctrl+Shift+T`, and `Ctrl+Tab` all work, producing a functioning tmux-style multiplexer in a single window.

**Exit criteria:**
- `Ctrl+Shift+H` splits the active pane horizontally (side-by-side). `Ctrl+Shift+V` splits vertically (stacked). Each pane has an independent PTY session.
- `Ctrl+Tab` / `Ctrl+Shift+Tab` cycle focus through panes in depth-first left-to-right layout tree order as specified in `docs/04-ux-config.md` §5.
- `Ctrl+Shift+T` opens a new tab. `Ctrl+Shift+W` closes the active pane (or the tab if it is the last pane).
- `Ctrl+Z` zooms the active pane to fill the window; pressing it again restores the layout. Window title shows `[ZOOM]` during zoom.
- Pane borders render with a 1px line in `cursor` color (active) and `bright_black` (inactive).
- Window resize correctly propagates through the layout tree via `reflow()`, calling `pty.resize()` on every leaf session with correct pixel dimensions.
- Two panes running simultaneously do not interfere: output to pane A does not affect pane B's grid.
- The tab bar renders when more than one tab is open, showing tab index and title.

**Crates modified:** `gunter-core` (Layout tree, Tab, WindowState), `gunter-renderer` (multi-pane render loop, pane border rendering, tab bar), `gunter-app` (split/tab/zoom action dispatch)

**Key technical work:**

- **Layout tree (`gunter-core/src/layout.rs`).** Implement `Layout`, `Axis`, and `Rect` as specified in SPEC.md. Implement `reflow(layout, rect, sessions)` as shown in the SPEC Phase 3 code. Implement `layout_leaves(layout) -> Vec<Uuid>` for cycle-order navigation (depth-first left-to-right). Implement `layout_find_adjacent(layout, id, direction) -> Option<Uuid>` for directional navigation (used by `Ctrl+H`/`Ctrl+L` directional focus — v2 feature, but the tree walk function is needed now for split insertion).

- **Tab and WindowState types (`gunter-core/src/session.rs`).** Implement `Tab { id, title, layout, zoom: Option<Uuid> }` and `WindowState { tabs: Vec<Tab>, active: usize, sessions: HashMap<Uuid, Session> }`. Implement `WindowState::active_session() -> &Session`, `WindowState::split(axis)`, `WindowState::new_tab()`, `WindowState::close_pane(id)`, and `WindowState::zoom_toggle()` as methods. These are pure data-structure operations; no GPU or PTY concerns here.

- **Split action (`gunter-app/src/actions.rs`).** When `Action::SplitHorizontal` fires: call `window_state.split(Axis::Horizontal)` which (a) spawns a new `Session` using the same shell command as the parent, (b) inserts a `Layout::Split` node replacing the current `Layout::Leaf`, and (c) calls `reflow()` to resize both sessions. The new pane inherits the parent pane's shell and CWD (via `/proc/self/cwd` on Linux or `GetCurrentDirectory` on Windows — best-effort; fallback to home dir).

- **Zoom overlay (`gunter-app/src/actions.rs`).** `Action::ZoomPane`: call `window_state.zoom_toggle()` which toggles `tab.zoom`. In the renderer, if `tab.zoom == Some(id)`, render only that session at the full window rect. Set `all_dirty = true` on both zoom-in and zoom-out (the zoomed pane resizes; the restored panes resize back). Window title updates to include `[ZOOM]` during zoom as specified in `docs/04-ux-config.md` §7.

- **Multi-pane renderer (`gunter-renderer/src/frame.rs`).** Extend `Renderer::render()` to iterate over all visible panes (leaves in the active tab's layout tree, or the single zoomed pane). For each pane: compute its pixel `Rect`, build `GlyphInstance`s from its `Grid` relative to that rect's origin, append to a single combined instance buffer. The background pass and glyph pass each draw all panes in one draw call (all instances in one buffer).

- **Pane border rendering (`gunter-renderer/src/borders.rs`).** After the two-pass glyph render, draw pane dividers as 1px horizontal/vertical lines. Use a separate simple quad draw call (solid color, no atlas). Active pane border: `cursor` color. Inactive: `bright_black`. Implement `Renderer::render_borders(layout, active_id, rect)` which walks the layout tree and emits border quads.

- **Tab bar rendering (`gunter-renderer/src/tabbar.rs`).** Render a top bar showing tab index and title when `tabs.len() > 1`. Tab bar height = one cell height. Active tab uses `selection` background; inactive tabs use `background`. The bar reduces the available pane area by its height — factor this into the `reflow` pixel rect calculation. Tab titles are derived from the active pane's window title (set via OSC 2 escape sequence, parsed by the VTE performer).

**Depends on:** v0.3.0 interaction model. `WindowState` replaces the single `Session` that v0.3.0 app.rs held directly.

**Risks / open questions:**
- PTY CWD inheritance on Windows via `GetCurrentDirectory` is unreliable across process boundaries. The new pane will likely open in the Gunter process's CWD, not the shell's CWD. Document as a known limitation; fix via OSC 7 (working directory notification) in v0.5.0 or v0.6.0.
- The tab bar reduces usable pane height by one row. This triggers a `pty.resize()` on all sessions when the first tab is created. Some shells redraw their prompt on resize — this is correct behavior but may look like a flicker. No mitigation needed; it is expected.
- Pane border click-drag for resize (from `docs/04-ux-config.md` §12): defer to v1.0.0. In v0.4.0, splits use a fixed 50% ratio.

**Why this version boundary:** This is the multiplexer milestone. After v0.4.0, Gunter can replace tmux for the basic split/tab workflow. The layout tree is the most structurally significant addition — everything after this version adds features on top of a proven data model.

---

## v0.5.0 — Multi-Shell and Socket Server

**Goal:** Any configured shell (PowerShell, CMD, custom) opens in new panes, and `gunter attach <id>` from a second terminal window joins an existing session over a named pipe / Unix socket.

**Exit criteria:**
- `shell.program = "pwsh.exe"` in config opens PowerShell panes correctly; `"cmd.exe"` opens CMD. The shell is read from a `ShellConfig` struct in `gunter-core` (not hardcoded in app.rs).
- `cargo run -- --server` starts Gunter in daemon mode: no window, just the socket server running and a single WSL session available for attachment.
- Running `gunter attach <session-id>` from a second terminal window connects to the running session, and input/output are multiplexed correctly (both terminals see the same output; input from either reaches the shell).
- Session IDs are printed to stderr on session creation and are stable `Uuid` values.
- The socket path is `\\.\pipe\gunter-{id}` on Windows and `/tmp/gunter-{id}.sock` on Unix/WSL.
- The protocol is a raw byte stream (stdin in, stdout out) — no framing. A plain `nc` or `socat` can attach.
- `gunter list` prints active session IDs to stdout.

**Crates modified:** `gunter-core` (server, shell config), `gunter-app` (CLI argument parsing, shell selection UI)

**Key technical work:**

- **Shell config and selection (`gunter-core/src/config.rs`).** Define `ShellConfig { program: String, args: Vec<String> }`. `Session::spawn` accepts a `ShellConfig` instead of a hardcoded shell string. In the app, the `ShellConfig` comes from the loaded `GunterConfig` (stub config struct for now; full TOML wiring in v0.6.0). Default: `wsl.exe` as before.

- **Multi-shell new-pane UX (`gunter-app/src/actions.rs`).** When splitting, the new pane inherits the `ShellConfig` of the split source pane. No shell-picker UI in v0.5.0 — that is v2. Future: `Ctrl+Shift+T` could offer a shell selection list. The shell config is per-window for now (all panes use the same shell program; each pane can have different args via config extension in v2).

- **Socket server (`gunter-core/src/server.rs`).** Implement `SessionServer::start(session_id: Uuid)` which binds a platform-specific socket path and spawns a tokio task to accept connections. On each connection: spawn a bidirectional relay task: reads from the socket stream → write to `session.tx` (PTY stdin), and reads from a new `session.pty.try_clone_reader()` → write to the socket stream. The raw byte protocol (no framing) matches the description in SPEC.md Phase 4. Use `tokio::net::UnixListener` on Unix and `tokio::net::windows::named_pipe::ServerOptions` on Windows.

- **`gunter attach` subcommand (`gunter-app/src/cli.rs`).** Parse `--attach <id>` or `attach <id>` as a CLI subcommand using `clap`. Connect to the session socket, then enter a raw-mode PTY passthrough: forward stdin to the socket stream, forward socket stream to stdout. Use `crossterm::terminal::enable_raw_mode()` for the attaching terminal. This is a thin client — no GPU, no window.

- **`gunter list` subcommand (`gunter-app/src/cli.rs`).** Scan for socket files matching `gunter-*.sock` or `\\.\pipe\gunter-*` and print their IDs. Alternatively, maintain a registry file at `~/.local/share/gunter/sessions.json` updated on session create/destroy.

- **`--server` mode (`gunter-app/src/main.rs`).** When `--server` is passed: initialize tokio runtime, create a `GunterConfig` from defaults, spawn one `Session` with the configured shell, start the `SessionServer`, and block on the server listener. No winit event loop, no window. The server runs until the PTY child exits or SIGINT is received.

**Depends on:** v0.4.0 `WindowState` and `Session` model. The server wraps `Session` instances that already exist in `WindowState::sessions`.

**Risks / open questions:**
- Named pipe creation on Windows requires careful ACL settings to allow other processes to connect. `tokio::net::windows::named_pipe` handles this, but test with a second process connecting as a different user.
- The socket relay uses `session.pty.try_clone_reader()` — verify that `portable-pty` supports multiple concurrent readers on ConPTY. If not, route through a `broadcast` channel instead of a direct reader clone.
- `--server` mode with no window means Gunter cannot be killed by closing a window. Handle SIGINT / `CtrlC` via `tokio::signal` to clean up sockets and kill PTY children gracefully.
- `clap` adds a compile-time dependency. Confirm the binary size impact stays under the 20 MB target.

**Why this version boundary:** The socket server is an optional extensibility feature that does not affect the core terminal UX. It is cleanly separable from the multiplexer work of v0.4.0. Completing it unlocks Neovim and VS Code integration use cases without blocking the config work in v0.6.0.

---

## v0.6.0 — Config System and Hot-Reload

**Goal:** All behavior is driven by `~/.config/gunter/config.toml`; changing `font.size` or `theme.name` applies immediately without restarting.

**Exit criteria:**
- On first run (no config file), Gunter generates `~/.config/gunter/config.toml` with all defaults and `# OPTIONAL:` comments as specified in `docs/04-ux-config.md` §14.
- All config keys from `docs/04-ux-config.md` §1 work: `[shell]`, `[font]`, `[theme]`, `[scrollback]`, `[keys]`.
- Changing `font.size` in the config while Gunter is running: the glyph atlas rebuilds at the new size, cells resize, layout reflows, and the display updates within one render frame after the 200ms debounce window.
- Changing `theme.name` swaps the color palette without rebuilding the atlas.
- Changing `[keys]` rebinds actions without restart.
- An invalid config file (parse error) is logged to stderr; Gunter continues with the last valid config ("last good config" policy from `docs/04-ux-config.md` §2).
- Atom One Dark ships as a built-in theme compiled into the binary via `include_str!`. User themes can be placed in `~/.config/gunter/themes/` and referenced by name.
- The font fallback chain (`font.fallback = [...]`) is passed to `wezterm-font` and functions correctly: CJK characters fall back to Noto Sans CJK, emoji to Segoe UI Emoji.

**Crates modified:** `gunter-core` (config types, hot-reload watcher), `gunter-renderer` (theme uniform buffer, atlas rebuild API), `gunter-input` (InputMap wired to config), `gunter-app` (config load on startup, hot-reload dispatch)

**Key technical work:**

- **Config types (`gunter-core/src/config.rs`).** Define `GunterConfig`, `ShellConfig`, `FontConfig`, `ThemeConfig`, `ScrollbackConfig`, `KeysConfig` with `serde::Deserialize`. The `config_version: u32` field is validated at load time (must equal 1 in v0.6.0; migration is v2). Implement `GunterConfig::load_or_generate(path) -> GunterConfig` which reads the file if present, generates a default if absent.

- **Theme loading (`gunter-core/src/theme.rs`).** Define `Theme` struct (the 16 ANSI colors + background/foreground/cursor/selection, all as `[f32; 4]` linear RGBA). Implement `Theme::from_toml_str(s: &str) -> Result<Theme>`. At startup: search user theme directory first, then fall back to built-in themes embedded via `include_str!`. The `atom-one-dark.toml` from `assets/themes/` is compiled into the binary. Color hex → linear float conversion uses `srgb_hex_to_linear` from `gunter-renderer` (or a shared utility in `gunter-core`).

- **Hot-reload watcher (`gunter-core/src/reload.rs`).** Spawn a tokio task using `notify::RecommendedWatcher` (maps to `ReadDirectoryChangesW` on Windows, inotify on Linux) watching `~/.config/gunter/config.toml`. Debounce 200ms as specified in `docs/04-ux-config.md` §2 (use `notify-debouncer-mini` or a manual `tokio::time::sleep`). On change: re-parse config, diff against previous, send a `ConfigChange` enum over a `mpsc::Sender<ConfigChange>` to the app's event loop.

- **ConfigChange dispatch (`gunter-app/src/app.rs`).** The winit event loop polls the `ConfigChange` channel each frame. Handle:
  - `ConfigChange::Theme(new_theme)` → update the wgpu color uniform buffer; call `renderer.update_theme(theme)`.
  - `ConfigChange::FontSize(size)` → call `renderer.rebuild_atlas(new_size)` which regenerates `GlyphAtlas` at the new point size, resizes cells, and sets `all_dirty = true` on all grids; calls `reflow()` to update all PTY sizes.
  - `ConfigChange::Keys(new_keys)` → replace `input_map` with a new `InputMap` built from `new_keys`.
  - `ConfigChange::Scrollback(lines)` → resize all `Grid::scrollback` VecDeques.

- **Font fallback wiring (`gunter-renderer/src/atlas.rs`).** Pass `FontConfig { family, fallback, size, line_height }` to `wezterm-font`'s font loader. The fallback chain is passed as a priority list. The atlas rasterizes each codepoint from the first font in the chain that covers it. Nerd Font private-use glyphs are in `family` (ComicShannsMono Nerd Font Mono) and need no fallback.

- **Keybinding config wiring (`gunter-input/src/lib.rs`).** Implement `InputMap::from_config(keys: &KeysConfig) -> InputMap`. Parse key strings (`"Ctrl+Shift+T"`) into `KeyCombo` values. Emit a `WARN` log for any key combo that matches a known shell conflict (per the conflict table in `docs/04-ux-config.md` §4). `"Ctrl+Z"` always gets the documented warning about SIGTSTP.

- **First-run config generation (`gunter-app/src/main.rs`).** At startup, call `GunterConfig::load_or_generate()`. If the file did not exist and was generated, log a message: `"Config written to ~/.config/gunter/config.toml"`. The generated config uses the default values from `config/default.toml` in the repo.

**Depends on:** v0.5.0 shell config (the stub `ShellConfig` is now replaced by the real config). All prior hardcoded values (Atom One Dark colors, ComicShannsMono size) are replaced by config reads.

**Risks / open questions:**
- Font family hot-reload is explicitly out of scope (requires restart, per `docs/04-ux-config.md` §2). Log a clear message: `"Font family change requires restart. Other font changes applied."` Do not attempt async font loading in v0.6.0.
- `notify` on Windows uses `ReadDirectoryChangesW` which fires on directory changes; filter to only the specific config file path to avoid spurious reloads.
- Atlas rebuild is synchronous on the main thread and may cause a brief frame drop (< 100ms for ComicShannsMono ASCII). This is acceptable for a config change operation. If profiling shows > 200ms, move the rasterization to `spawn_blocking` and apply the new atlas on the next frame.
- `wezterm-font` font loader path resolution on Windows: confirm that `wezterm-font` can find fonts via `DirectWrite` font enumeration given a family name string. If the font name does not match DirectWrite's canonical name, the lookup silently falls back to a system font without error. Add a startup log line printing the resolved font path.

**Why this version boundary:** The config system touches every other subsystem (renderer, input, core) but does not add new user-visible features beyond the configuration itself. Completing it makes Gunter fully customizable and lays the groundwork for the v1.0 hardening pass.

---

## v1.0.0 — Production Hardening

**Goal:** Gunter meets all stated performance targets, copy/paste and mouse support are complete, pane border resize works, and a Windows installer is available — this is the public release.

**Exit criteria:**
- Input latency (keystroke to pixel change) is < 10ms on a 60Hz display, measured with a hardware latency tool or the `termbench` approach.
- `cat /dev/urandom | head -c 100M` streams at > 100 MB/s through the PTY without dropping frames or OOMing (measured via PTY byte counter in the status bar).
- Memory RSS for a single WSL pane with 5000-line scrollback is < 50 MB.
- Binary size (`cargo build --release`) is < 20 MB.
- Cold start to interactive prompt is < 200ms (timed from process launch to first shell prompt rendered).
- Mouse scroll in the viewport scrolls the scrollback buffer smoothly.
- Click-drag text selection copies to the OS clipboard on release.
- `Ctrl+Shift+V` pastes with bracketed paste wrappers (already in v0.3.0; regression-tested here).
- Pane border click-drag resizes the split ratio interactively.
- OSC 52 sequences from inside the PTY (e.g. Neovim `set clipboard=unnamedplus`) write to the Windows clipboard.
- A Windows installer (NSIS or WiX via `cargo-wix`) is produced by CI and installable without Rust toolchain.
- `cargo install gunter` works from crates.io.
- `CHANGELOG.md` and `README.md` are written and accurate.

**Crates modified:** All crates (performance pass), `gunter-renderer` (scrollback rendering, pane border drag), `gunter-term` (OSC 52 handler), `gunter-app` (mouse selection, drag resize, packaging scripts)

**Key technical work:**

- **Input latency audit.** Profile the key-down → PTY-write → VTE-parse → dirty-flag → render pipeline end-to-end. The bottleneck is likely one of: winit event dispatch latency, `RwLock<Grid>` write contention, or vertex buffer upload. If `RwLock` contention is the problem, switch to the double-buffer scheme described in `docs/04-ux-config.md` §13: VTE task writes to a back buffer; render task swaps atomically once per frame. Target: < 1ms from key event to PTY write, < 5ms from PTY write to dirty flag set, < 5ms from dirty flag to pixel.

- **Throughput optimization.** Profile `cat large-file` at 100 MB/s. The VTE parser (`vte` crate) is the expected bottleneck at this throughput. Ensure the PTY reader uses a large buffer (64 KB, not 4 KB) to amortize the per-`advance` call overhead. If `Vec<bool>` dirty flag scanning is slow at 11,000 cells/frame, replace with a `bitvec` crate bitset — the decision doc (`02-renderer.md` §4) explicitly deferred this optimization to after profiling. Verify that `all_dirty = true` path (used during high-throughput scrolling) uploads the instance buffer in one `write_buffer` call rather than per-range.

- **Scrollback UX.** Implement mouse wheel scrollback: `WindowEvent::MouseWheel` → increment `scroll_offset` → full instance buffer rebuild from scrollback rows (the design is in `docs/02-renderer.md` §7). Implement a scroll indicator (a thin vertical bar on the right edge showing scroll position, rendered as a background quad in `cursor` color). Clamping: `scroll_offset` cannot exceed `scrollback.len()`.

- **Mouse text selection.** On `WindowEvent::MouseInput` left-button down: record the cell coordinate under the cursor as `selection_start`. On `MouseMove` with left button held: update `selection_end`. Highlight selected cells by inverting their `fg`/`bg` in the instance builder (set `CellFlags::INVERSE`). On button release: extract the text from the selected cell range and write it to the OS clipboard using `arboard::Clipboard::new().set_text(text)`.

- **OSC 52 clipboard handler (`gunter-term/src/osc.rs`).** In the VTE performer's `osc_dispatch` handler, detect OSC 52 sequences (`\e]52;c;<base64>\a`). Decode the base64 payload and write it to the OS clipboard via `arboard`. This enables Neovim's `set clipboard=unnamedplus` to work through Gunter without any OS-level clipboard proxy.

- **Pane border drag resize (`gunter-app/src/input.rs`).** On `MouseMove`: if the cursor is within 4px of a pane border (detected by walking the layout tree's split points against the cursor position), change the cursor icon to a resize cursor. On left-button drag starting from a border: compute the new `ratio` from the cursor delta and call `WindowState::update_split_ratio(split_id, new_ratio)`, followed by `reflow()`. Clamp ratio to `[0.1, 0.9]` to prevent panes from collapsing to zero width.

- **Performance targets verification.** Implement a `--bench` flag that runs a self-contained throughput and latency measurement (feed known byte sequences through the PTY pipeline, measure frame times) and prints a report. This becomes the CI regression gate.

- **Packaging.** Add a `Makefile` or `xtask` with targets:
  - `cargo build --release` — standard binary.
  - `cargo wix` (via `cargo-wix` crate) — produces a `.msi` Windows installer.
  - `cargo publish` — publish to crates.io (requires `gunter-core`, `gunter-term`, `gunter-input` to also be publishable or made private dependencies).
  - Document `cargo install gunter` in `README.md`.

**Depends on:** v0.6.0 complete config system. All prior versions. This version does not add new architecture — it optimizes, completes partial features, and ships.

**Risks / open questions:**
- `arboard` clipboard on Windows may require `COM` initialization on the thread calling clipboard APIs. Confirm this works from the winit event loop thread (which is already the main thread on Windows). If COM init is needed, call `CoInitializeEx(NULL, COINIT_APARTMENTTHREADED)` early in `main()`.
- Binary size: `wezterm-font` vendoring is the largest contributor. Run `cargo bloat --release` to identify the top contributors. If `wezterm-font` alone exceeds 10 MB, evaluate `cosmic-text` as a replacement (flagged as a contingency in `docs/02-renderer.md` §2).
- `cargo wix` requires the WiX toolset installed on the build machine. Add a CI step that installs WiX before packaging.
- OSC 52 base64 decoding: very large clipboard payloads (> 1 MB) can cause a frame drop if decoded on the VTE parser task. Offload to `tokio::spawn_blocking` for payloads > 64 KB.
- The 100 MB/s throughput target is for PTY reading — not for display rendering. At 100 MB/s the terminal will be in `all_dirty = true` mode continuously (the output scrolls so fast no individual cell state matters). Confirm the instanced draw path does not degrade under continuous full-redraw conditions.

**Why this version boundary:** This is the public release. Every prior version was a dev milestone; this is the first version that ships to users outside the development team. The exit criteria are measurable, objective, and cover the full scope stated in `docs/01-overview.md` §Success Criteria. After v1.0.0, new features (v2: animations, scripting, session persistence, 120fps, scrollback search) follow the same versioned plan pattern.

---

## Crate Interface Summary at v1.0.0

This table shows the primary public interfaces between crates at the end of each version. `gunter-app` depends on all other crates; no other crate depends on `gunter-app`.

```
gunter-core  ──── Grid, Cell, CellFlags, Color, Rect
             ──── Session::spawn(shell, size) -> Session
             ──── WindowState (tabs, sessions, layout tree)
             ──── GunterConfig, ShellConfig, FontConfig, ThemeConfig, KeysConfig
             ──── Theme (linear RGBA color arrays)
             ──── SessionServer::start(session_id)
             ──── ConfigReloadWatcher::new(path) -> mpsc::Receiver<ConfigChange>

gunter-term  ──── GridPerformer (implements vte::Perform)
             ──── MouseReportingMode
             ──── OscHandler::handle_osc52(payload, clipboard)

gunter-input ──── Action (enum)
             ──── KeyCombo (Hash + Eq)
             ──── InputMap::from_config(keys) -> InputMap
             ──── InputMap::lookup(combo) -> Option<Action>

gunter-renderer
             ──── Renderer::new(window, config) -> Renderer
             ──── Renderer::render(window_state)
             ──── Renderer::resize(new_size)
             ──── Renderer::update_theme(theme)
             ──── Renderer::rebuild_atlas(font_config)

gunter-app   ──── main() — winit event loop, wires all crates
             ──── CLI: `gunter [--server] [attach <id>] [list]`
```

`gunter-app` is the only binary crate. All others are library crates. `gunter-core` and `gunter-term` have no dependency on `gunter-renderer` or `gunter-input` — the data flow is strictly one-way: `gunter-term` writes to `Grid`, `gunter-renderer` reads from `Grid`.

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `wezterm-font` vendoring too complex | Medium | Blocks v0.2.0 | Fallback: `fontdue` for ASCII, defer Nerd Font to v0.6.0 |
| `wgpu` DX12 surface fails on build machine | Low | Blocks v0.2.0 | Vulkan fallback; document min OS version (Win10 1903+) |
| `RwLock<Grid>` contention at 100 MB/s | Medium | Blocks v1.0.0 perf target | Double-buffer scheme (back buffer swap per frame) |
| `portable-pty` ConPTY behavior differs between Windows versions | Low | Blocks v0.1.0 | Test on Win10 1809 and Win11; file issues upstream |
| Binary size > 20 MB due to `wezterm-font` | Medium | Blocks v1.0.0 packaging | `cargo bloat` audit; evaluate `cosmic-text` replacement |
| `arboard` COM init requirement on Windows | Low | Blocks v1.0.0 copy/paste | Call `CoInitializeEx` early in `main()` |
