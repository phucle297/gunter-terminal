# Gunter — Config/UX Design Decisions

See SPEC.md for config schema, keybindings, and Atom One Dark palette.

---

## Why TOML

YAML: indentation errors, colon-quoting, `serde_yaml` correctness bugs. KDL: near-zero ecosystem, lagging serde support. Lua (`mlua`): +2 MB binary for declarative needs. JSON: no comments. TOML is already in the stack, zero friction for the target user. No include/import — solved by theme-by-name reference instead of inline colors.

---

## Hot Reload Architecture

`notify` wraps inotify/FSEvents/ReadDirectoryChangesW — needed because Gunter targets Windows-native (DX12). SIGHUP is Unix-only and requires signal-safe code. Polling adds ≥1 s lag and burns CPU.

Watcher runs in a **dedicated tokio task**. Flow: `file change → debounce 200 ms → re-parse TOML → diff Config → dispatch to subsystems`.

**Debounce rationale:** editors (Neovim, VS Code) write atomically — temp file + rename. `notify` fires two events (unlink + create) or one (rename). 200 ms absorbs both; imperceptible to humans.

**Partial apply — what hot-reloads without restart:**
- `theme.name` / inline `[colors]` → reload wgpu color uniform buffer
- `font.size`, `font.line_height` → atlas rebuild + reflow
- `[keys]` → replace `HashMap<KeyCombo, Action>` in gunter-input
- `scrollback.lines` → in-place `VecDeque` resize

**Requires restart (v1):** `shell.program/args` (PTY already spawned), `font.family` (sync font load at startup; async is v2), `config_version` bump (migration runs only at startup).

**Last-good-config policy:** parse failure → log error, keep last valid config. No crash, no revert prompt.

v1 watches only `~/.config/gunter/config.toml`. `~/.config/gunter/themes/` watcher is v2.

---

## Keybinding System

Actions are a **closed Rust enum** in `gunter-input`. WezTerm uses string-based dynamic dispatch for Lua action names — Gunter v1 has no scripting so the enum is strictly better: compile-time exhaustiveness, O(1) `HashMap<KeyCombo, Action>` lookup per keypress.

Conflict handling: last definition wins, warning to stderr. Known shell-shortcut conflicts emit a named startup warning.

---

## Why These Defaults (and What Was Rejected)

**Ctrl+Shift+T / Ctrl+Shift+W:** match kitty exactly. `Ctrl+T` = readline `transpose-chars`; `Ctrl+W` = readline `kill-word` (used constantly by bash/zsh). The Shift makes them unambiguously multiplexer actions and eliminates both readline conflicts.

**Ctrl+Shift+H / Ctrl+Shift+V for splits:** without Shift, `Ctrl+H` = ASCII 0x08 (backspace) in many terminals. Shift disambiguates. H = horizontal layout growth (panes side-by-side), V = vertical growth (panes stacked).

Axis naming: Gunter follows i3 convention — `split_h` = side-by-side (vertical divider), `split_v` = stacked (horizontal divider). The axis is the direction of layout growth, not divider orientation. **This conflicts with tmux's naming and must be documented prominently.**

**Ctrl+Tab / Ctrl+Shift+Tab for pane focus:** universal browser/IDE "cycle tabs" muscle memory. `Ctrl+L` = readline clear-screen. `Ctrl+H` = backspace. `Ctrl+Arrow` = word-jump in readline and most editors. All three conflict with fundamental shell interactions and were removed.

**Ctrl+Z for zoom:** hard SIGTSTP conflict. Gunter intercepts at window layer only, not pane layer. Mitigation: `# WARNING: conflicts with SIGTSTP` comment in default.toml. v2: detect shell via OSC 133, pass through when not in multiplexer mode.

**No leader key (v1):** `Ctrl+B <key>` is conflict-free but two-keystroke. Gunter prefers single-chord + explicit conflict management. Leader is v2 opt-in via `keys.leader`.

---

## Pane Cycle Order

Depth-first left-to-right traversal of the `Layout` tree. Spatial directional nav (`Ctrl+Shift+L/H/J/K`) is v2 — requires geometric adjacency, not just tree order. Wrap-around (last→first) matches browser tab cycling; `keys.wrap_navigation = false` is v2.
