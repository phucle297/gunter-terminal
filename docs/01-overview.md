# Gunter — Project Overview

> See SPEC.md for crate stack, data types, config schema, and build phases.

Gunter is a native Rust terminal emulator for Windows/WSL2. The core problem: every existing
option is either slow (Electron/DOM), platform-locked, or requires separate multiplexer
plumbing. Gunter collapses PTY management, GPU rendering, and tmux-style multiplexing into
one ~15 MB binary with no runtime dependencies.

---

## How the Crates Relate (Conceptually)

```
PTY stdout
  → tokio read task
  → vte::Parser (gunter-term)       ← pure state machine, no I/O
  → Grid mutations (dirty flags set)
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

`gunter-term` is intentionally I/O-free — it only transforms bytes into grid state. This
makes it independently testable. `gunter-renderer` never touches PTY or input; it only
consumes the dirty-cell diff. `gunter-app` is the only crate that owns the winit loop and
wires everything together.

---

## Crate Decisions (Why This Stack)

| Crate | Why not alternatives |
|---|---|
| `wgpu` | Single codebase for DX12+Vulkan; safe Rust API; WSL2 GPU-PV compatible |
| `winit` | Only cross-platform window crate with first-class wgpu integration |
| `vte` | Zero-alloc, battle-tested (same as Alacritty); `termwiz` is heavier |
| `portable-pty` | Handles ConPTY on Windows; no alternative does this correctly in pure Rust |
| `tokio` | PTY read, config watch, socket server all need async; `async-std` has less ecosystem |
| `wezterm-font` | Handles Nerd Font PUA glyphs, font fallback, shaping; saves weeks vs `fontdue` alone |
| `serde` + `toml` | TOML = comments work; already used by Cargo; `serde_yaml` has known correctness issues |
| `notify` | Cross-platform (inotify/FSEvents/ReadDirectoryChangesW); polling is too slow |

---

## Key Conflict Philosophy

All default bindings are conflict-free with readline/shell except `Ctrl+Z` (SIGTSTP). Users
who rely on `fg`/`bg` must rebind `zoom_pane`. This is the only intentional tradeoff — the
muscle memory benefit of `Ctrl+Z` zoom outweighs forcing a config change on power users.

---

## Success Criteria

- Input latency < 10ms keystroke-to-render (kitty target: ~5ms)
- Throughput: 100 MB/s `cat large-file` without dropping frames
- Memory: < 50 MB RSS for a single WSL pane with 5000-line scrollback
- Binary size: ~15 MB release build
- Startup: < 200ms cold start to interactive prompt

---

## v1 Scope Boundary

Not in v1: session persistence, remote attach, macOS/Linux native builds (wgpu/portable-pty
support it; scheduling does not), scripting (no Lua/Python), animations.
