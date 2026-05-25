# Gunter — Project Overview & Roadmap

> Fast, GPU-accelerated terminal for Windows/WSL.  
> Speed first. Simple always. Inspired by kitty (GPU), Hyper (UX), tmux (multiplexing).

---

## What Gunter Is

A native Rust terminal emulator targeting Windows/WSL2 with:
- GPU-rendered text via `wgpu` (DX12 on Windows, no Electron, no DOM)
- tmux-style pane splits and tabs in a single binary
- Zero-config defaults, TOML hot-reload config
- ~15 MB binary, minimal RAM per pane
- Named pipe/socket attach for Neovim and VS Code integration

## What Gunter Is Not (v1)

- Not a drop-in tmux replacement (no session persistence, no remote attach)
- Not cross-platform beyond Windows/WSL in v1 (wgpu and portable-pty support macOS/Linux; native builds deferred post-v1)
- Not scriptable (no Lua, no Python — v2)
- Not animated (hyprland-style animations: v2)

---

## Architecture Snapshot

```
gunter/
├── crates/
│   ├── gunter-core/      PTY management, session model, layout tree, config types
│   ├── gunter-renderer/  wgpu surface, glyph atlas, GPU draw calls
│   ├── gunter-term/      vte parser → terminal grid state machine
│   ├── gunter-input/     keyboard/mouse → Action enum dispatch
│   └── gunter-app/       main binary — winit event loop, wires all crates
├── config/
│   └── default.toml
└── assets/themes/
    └── atom-one-dark.toml
```

Data flow:

```
PTY stdout
  → tokio read task
  → vte::Parser (gunter-term)
  → Grid mutations (Cell dirty flags set)
  → frame tick: diff dirty cells
  → Vec<GlyphInstance> (gunter-renderer)
  → wgpu vertex buffer upload (dirty range only)
  → instanced draw call → screen

Keyboard/mouse
  → winit event (gunter-app)
  → gunter-input: KeyCombo → Action
  → Action dispatch: SendToPane | NewTab | SplitH | ...
  → PTY stdin write or layout mutation
```

---

## Crate Decisions (Why This Stack)

| Crate | Role | Why not alternatives |
|---|---|---|
| `wgpu` | GPU rendering | Single codebase for DX12+Vulkan; safe Rust API; WSL2 GPU-PV compatible |
| `winit` | Window + events | Only cross-platform window crate with wgpu integration |
| `vte` | VT/ANSI parser | Zero-alloc, battle-tested (same as Alacritty); `termwiz` is heavier |
| `portable-pty` | PTY allocation | Handles ConPTY on Windows; no alternative does this correctly in pure Rust |
| `tokio` | Async runtime | PTY read, config watch, socket server all need async; `async-std` has less ecosystem |
| `wezterm-font` | Glyph atlas | Handles Nerd Font PUA glyphs, font fallback, shaping; saves weeks vs `fontdue` alone |
| `serde` + `toml` | Config | TOML = no comments problem (JSON); TOML already used by Cargo; `serde_yaml` has correctness history |
| `notify` | Config hot-reload | Cross-platform (inotify/FSEvents/ReadDirectoryChangesW); polling is too slow |
| `uuid` | Session IDs | Standard; no alternative needed |

---

## Default Keybindings

| Action | Key | Why |
|---|---|---|
| New tab | `Ctrl+Shift+T` | Matches kitty; avoids readline `transpose-chars` conflict |
| Close pane/tab | `Ctrl+Shift+W` | Matches kitty; avoids readline `kill-word` conflict |
| Split horizontal | `Ctrl+Shift+H` | H = horizontal layout growth direction |
| Split vertical | `Ctrl+Shift+V` | V = vertical layout growth direction |
| Next pane | `Ctrl+Tab` | Universal "next tab" muscle memory; zero shell conflicts |
| Prev pane | `Ctrl+Shift+Tab` | Universal "prev tab"; zero shell conflicts |
| Zoom pane | `Ctrl+Z` | Matches tmux `Ctrl+B z` model; **conflicts with SIGTSTP** — documented in config |

**Key conflict philosophy:** All default bindings are conflict-free except `Ctrl+Z` (SIGTSTP). Users who need `fg`/`bg` should rebind `zoom_pane`. See `docs/04-ux-config.md` §4 for full rationale.

---

## Build Phases

| Phase | Goal | Key Crates | Status |
|---|---|---|---|
| 1 | WSL shell → grid buffer (no window) | `portable-pty`, `vte`, `tokio` | Not started |
| 2 | GPU renderer, fonts, Atom One Dark | `wgpu`, `winit`, `wezterm-font` | Not started |
| 3 | Pane splits, tabs, keyboard nav | internal layout tree | Not started |
| 4 | Multi-shell, socket server attach | `tokio::net` | Not started |
| 5 | Config hot-reload, font fallback | `serde`, `toml`, `notify` | Not started |

**Phase order is mandatory.** Phase 1 validates the PTY→VTE→Grid pipeline before any GPU work. A broken grid draws incorrectly at 60fps — GPU doesn't fix logic bugs.

---

## Planning Documents

| File | Contents |
|---|---|
| `docs/01-overview.md` | This file — architecture snapshot, crate decisions, phase map |
| `docs/02-renderer.md` | GPU renderer deep-dive: wgpu, glyph atlas, instanced draw, dirty tracking |
| `docs/03-core-pty.md` | PTY/VTE/Grid deep-dive: async model, layout tree, session lifecycle |
| `docs/04-ux-config.md` | UX, keybindings, config system, hot-reload, theme system |

---

## Success Criteria

- **Input latency** < 10ms keystroke-to-render (kitty target: ~5ms)
- **Throughput** 100 MB/s `cat large-file` without dropping frames
- **Memory** < 50 MB RSS for a single WSL pane with 5000-line scrollback
- **Binary size** ~15 MB release build
- **Startup** < 200ms cold start to interactive prompt
