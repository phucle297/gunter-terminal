# Gunter

Fast, GPU-accelerated terminal emulator for Windows/WSL2. Inspired by kitty and tmux.

- **GPU rendering** via `wgpu` (DX12/Vulkan) — zero DOM, no Electron
- **tmux-style multiplexing** — pane splits, tabs, built-in
- **Multi-shell** — WSL, PowerShell, CMD, any POSIX shell
- **Embeddable** — named pipe/Unix socket for external attach (`gunter attach <id>`)
- **Lean** — ~15 MB binary, <50 MB RSS per pane

Default theme: Atom One Dark. Default font: ComicShannsMono Nerd Font Mono.

---

## Status

| Version | Phase | Status |
|---|---|---|
| v0.1.0 | WSL shell → grid buffer (headless) | planned |
| v0.2.0 | GPU renderer, fonts, Atom One Dark | planned |
| v0.3.0 | Keyboard input, interactive terminal | planned |
| v0.4.0 | Pane splits, tabs | planned |
| v0.5.0 | Multi-shell, socket server | planned |
| v0.6.0 | Config hot-reload | planned |

---

## Install

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/phucle297/gunter-terminal/develop/install.ps1 | iex
```

Downloads `gunter.exe` from the latest GitHub Release, verifies SHA256, and adds it to your user PATH.

To pin a version: `$env:GUNTER_VERSION = "v0.2.0"; irm ... | iex`

**Manual install:** Download `gunter-windows-x86_64.zip` from [Releases](https://github.com/phucle297/gunter-terminal/releases), unzip, add the folder to PATH.

### Build from source

> Requires: Rust stable, MSVC toolchain, DX12-capable GPU (Windows) or Vulkan/GL (Linux)

```bash
cargo build --release -p gunter-app
```

---

## Usage

```bash
# Launch default shell (wsl.exe)
gunter

# Attach to existing session
gunter attach <session-id>
```

**Default keybinds:**

| Key | Action |
|---|---|
| `Ctrl+Shift+T` | New tab |
| `Ctrl+Shift+W` | Close tab |
| `Ctrl+Shift+H` | Split horizontal |
| `Ctrl+Shift+V` | Split vertical |
| `Ctrl+Tab` | Next pane |
| `Ctrl+Shift+Tab` | Prev pane |
| `Ctrl+Z` | Zoom pane |

---

## Config

`~/.config/gunter/config.toml`

```toml
[shell]
program = "wsl.exe"

[font]
family = "ComicShannsMono Nerd Font Mono"
size   = 14.0

[theme]
name = "atom-one-dark"

[scrollback]
lines = 5000
```

---

## Crate Layout

```
crates/
├── gunter-core/      # Grid, Cell, session model, config types
├── gunter-renderer/  # wgpu surface, glyph atlas, draw calls
├── gunter-term/      # vte parser → terminal grid state
├── gunter-input/     # keyboard/mouse → action mapping
└── gunter-app/       # main binary — winit loop, wires all crates
```

See [SPEC.md](SPEC.md) for full architecture, data types, and build phase details.
