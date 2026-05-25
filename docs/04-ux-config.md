# Gunter — UX, Config, and Feature Design

> Authoritative decisions for every user-facing system in Gunter.
> Each section states the choice, explains why the alternatives were rejected, and marks v1 vs future scope.

---

## Summary

Gunter targets three values in order: **input latency**, **UX clarity**, **hackability**. Every decision below is evaluated against that hierarchy. When two options tie on latency, clarity wins. When they tie on clarity, the more hackable option wins.

The target user is someone already running Neovim or a tiling WM (hyprland, i3, sway) inside WSL2 on Windows. They expect:
- Zero-friction keyboard-driven multiplexing.
- Config that reads like a document, not a program.
- No magic — what's in the config is what the terminal does.

---

## 1. Config Format

### Decision: TOML at `~/.config/gunter/config.toml`

TOML is the config format. Single file. Versioned with a `config_version` key for future migrations. Two supplementary file types exist: theme files (`assets/themes/*.toml`) and a future keybind override file.

```toml
config_version = 1

[shell]
program = "wsl.exe"
args    = []

[font]
family      = "ComicShannsMono Nerd Font Mono"
fallback    = ["Noto Sans CJK", "Segoe UI Emoji"]
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

### Why not YAML

YAML's indentation-sensitive syntax creates copy-paste errors. Strings with colons require quoting. Multi-document syntax (`---`) is a footgun. The `serde_yaml` crate has had correctness issues (1.0 merge drama, silent truncation of large integers). TOML has none of these problems and `serde` + `toml` is already in the stack.

### Why not KDL

KDL is elegant but has near-zero ecosystem adoption in 2025. Users googling "how to configure gunter" expect either TOML or YAML. `kdl` crate serde support lags behind `toml`. Not worth the onboarding friction for a 0.x project.

### Why not Lua (kitty-style)

Lua gives programmatic power (conditional keybinds, dynamic themes) but requires embedding a Lua runtime (`mlua`), adding ~2 MB to the binary and significant maintenance surface. Gunter's v1 config needs are fully met by declarative TOML. If scripting becomes necessary, Lua is the correct v2 choice — the config key space should be designed so a Lua shim can produce the same schema.

### Why not JSON

JSON has no comments. Terminal config files need comments. Rejected.

### Why not command-line-only (Alacritty pre-config era)

CLI flags do not survive restarts. A user setting font size from a flag would have to script that into their shell profile, re-introducing the "config file" problem but worse. Rejected.

### Why not a single binary-blob / database (Ghostty approach)

Ghostty uses a custom line-oriented format. It's fast to parse but non-standard. TOML is the lingua franca of Rust project configs (`Cargo.toml` itself). Users already know it.

### Trade-offs accepted

- TOML arrays-of-tables syntax (`[[keys.bindings]]`) is awkward for large keybind tables. Accepted: v1 keybinds are a flat `[keys]` table, which is clean. Complex binding tables are a v2 problem.
- TOML does not support includes/imports. Accepted: theme is a separate file referenced by name, not inlined. That covers the main composition use case.

---

## 2. Hot Reload Strategy

### Decision: `notify` crate watching `~/.config/gunter/config.toml`, debounced 200ms, partial application

The `notify` crate provides a cross-platform abstraction over inotify (Linux), FSEvents (macOS), and ReadDirectoryChangesW (Windows). The watcher runs in a dedicated tokio task. On a change event it re-parses the config, diffs against the in-memory copy, and applies only what changed.

```
config file change
  → debounce 200ms (absorb rapid saves from editors writing atomically)
  → re-parse TOML
  → diff: Config { old } vs Config { new }
  → dispatch change set to affected subsystems
```

**Safe to hot-reload (no restart required):**

| Config key | Effect | Method |
|---|---|---|
| `theme.name` | Swap color uniforms in wgpu | Reload uniform buffer |
| `font.size` | Rebuild glyph atlas, resize cells, reflow | Atlas rebuild + reflow |
| `font.line_height` | Cell height recalculation, reflow | Reflow only |
| `[keys]` | Re-register action map in gunter-input | Replace HashMap |
| `scrollback.lines` | Trim or grow the VecDeque | In-place resize |
| `[colors]` (inline) | Same as theme swap | Reload uniform buffer |

**Requires restart (v1 limitation):**

| Config key | Reason |
|---|---|
| `shell.program` / `shell.args` | PTY already spawned. Cannot re-exec shell mid-session. |
| `font.family` (different font file) | Font file load is synchronous at startup; async font loading is v2. |
| `config_version` bump | Migration logic runs only at startup. |

### Why not inotify directly

`libc::inotify_init` is Linux-only. Gunter targets Windows/WSL. A direct inotify binding would require a Windows stub. `notify` already does this portably.

### Why not polling

Polling at 1s intervals adds 1s maximum reload lag for a user who saved the config expecting instant feedback. Polling also burns CPU when the file has not changed. Event-based is strictly better.

### Why not SIGHUP

SIGHUP is Unix-only. Windows does not have POSIX signals in the same sense. Gunter targets Windows-native (DX12 surface via wgpu) — SIGHUP is not portable. Also, SIGHUP handling requires signal-safe code, which is harder to reason about than a channel-based design.

### Debounce rationale

Many editors (Neovim, VS Code) write config files atomically: write to a temp file, then rename. `notify` may fire two events (unlink + create) or one (rename). A 200ms debounce absorbs both patterns cleanly. 200ms is imperceptible to humans but catches all common editor save patterns.

### Trade-offs accepted

- Font family change requires restart. A user switching from `ComicShannsMono` to `JetBrains Mono` sees a "restart required" notification in the status bar (v2: async font load).
- Hot reload applies the diff in the main event loop tick. If the TOML parse fails (user mid-edit), Gunter logs the error but does not crash or revert — it keeps the last valid config. This is the "last good config" policy.

---

## 3. Keybinding System Design

### Decision: Enum-based actions, string-parsed bindings, no runtime scripting in v1

Actions are a closed Rust enum in `gunter-input`:

```rust
pub enum Action {
    NewTab,
    CloseTab,
    SplitHorizontal,
    SplitVertical,
    FocusNextPane,
    FocusPrevPane,
    ZoomPane,
    ScrollUp,
    ScrollDown,
    EnterScrollback,
    ExitScrollback,
    CopySelection,
    PasteClipboard,
    // ... extensible
}
```

The config `[keys]` table maps string keys (`"Ctrl+T"`) to action names (`"new_tab"`). At startup the string is parsed into a `KeyCombo { modifiers: Modifiers, key: Key }` and inserted into a `HashMap<KeyCombo, Action>`. Lookup on every keypress is O(1).

Key string format: `"Modifier+Key"` where modifiers are `Ctrl`, `Alt`, `Shift`, `Super`, and key is a named key (`T`, `Enter`, `F1`, etc.) or a Unicode character.

### Why not string-based action dispatch (WezTerm approach)

WezTerm uses string action names dispatched through a dynamic match — this supports Lua scripts returning action strings. Gunter v1 has no scripting, so a dynamic dispatch adds indirection with no benefit. The enum gives exhaustiveness checks at compile time: adding a new feature requires adding an enum variant, which the compiler forces the match arms to handle.

### Why not Lua scripting (kitty model)

kitty embeds Python for config and action scripting. This is powerful but complex. Gunter v1 is deliberately simple. The enum model can be wrapped in a Lua binding in v2 if demand exists — the action surface is the stable API.

### Conflict handling

When two bindings map to the same `KeyCombo`, the last definition in the config wins with a warning logged to stderr. This matches the "last wins" convention from CSS and most shell configs. The warning prevents silent conflicts.

When a binding conflicts with a known shell shortcut, Gunter emits a startup warning. Default bindings are chosen to avoid conflicts — `Ctrl+Tab`/`Ctrl+Shift+Tab` have no shell conflicts. If the user rebinds to a conflicting key (e.g. `next_pane = "Ctrl+L"`), they get:

```
WARN: Ctrl+L is bound to FocusNextPane but also used by readline (clear screen).
      Shell apps will not receive Ctrl+L while Gunter intercepts it.
```

### Trade-offs accepted

- Closed enum means users cannot bind arbitrary shell commands without a code change. Accepted for v1. The workaround is to bind a pane action that runs a shell command — that feature (bind to `ExecCommand`) is a v2 extension point.
- No chord/leader key in v1. tmux's `Ctrl+B <key>` model eliminates all conflicts but requires two-keystroke latency. Gunter prefers single-chord bindings and the associated conflict management overhead.

---

## 4. Default Keybinding Rationale

### Decision table

| Action | Gunter default | kitty | tmux | zellij | Conflict |
|---|---|---|---|---|---|
| New tab | `Ctrl+Shift+T` | `Ctrl+Shift+T` | `Ctrl+B c` | `Ctrl+T` | None |
| Close tab/pane | `Ctrl+Shift+W` | `Ctrl+Shift+W` | `Ctrl+B x` | `Ctrl+W` | None |
| Split horizontal | `Ctrl+Shift+H` | `Ctrl+Shift+\` | `Ctrl+B "` | `Ctrl+Shift+5` | None |
| Split vertical | `Ctrl+Shift+V` | `Ctrl+Shift+5` | `Ctrl+B %` | `Ctrl+Shift+2` | None |
| Focus next pane | `Ctrl+Tab` | `Ctrl+Shift+]` | `Ctrl+B →` | `Ctrl+→` | None significant |
| Focus prev pane | `Ctrl+Shift+Tab` | `Ctrl+Shift+[` | `Ctrl+B ←` | `Ctrl+←` | None significant |
| Zoom pane | `Ctrl+Z` | `Ctrl+Shift+F` | `Ctrl+B z` | `Ctrl+Z` | **SIGTSTP (suspend)** |

### Ctrl+Shift+T — why this over alternatives

`Ctrl+Shift+T` matches kitty exactly and avoids the `Ctrl+T` conflict with readline `transpose-chars`. The Shift modifier makes it unambiguously a terminal-multiplexer action rather than a shell passthrough. Browsers also use `Ctrl+Shift+T` for "reopen closed tab" — the muscle memory is compatible.

**Why not Ctrl+T:** `Ctrl+T` is `transpose-chars` in readline/bash. Zellij uses it but accepts the conflict. Gunter prefers zero conflicts for tab/pane management keys since these fire at the terminal layer and silently eat the keystroke from the shell.

### Ctrl+Shift+W — why this over alternatives

`Ctrl+Shift+W` matches kitty's close binding and avoids the `Ctrl+W` conflict with readline `kill-word` (delete word backward). `kill-word` is used constantly by bash/zsh users — intercepting it for close-tab is unacceptable.

**Why not Ctrl+W:** Too destructive to steal from the shell. A user typing fast could accidentally close a pane. The Shift requirement prevents accidental dismissal and eliminates the readline conflict entirely.

### Ctrl+Shift+H / Ctrl+Shift+V — why Shift

Without Shift, `Ctrl+H` is the binding for `prev_pane`. Adding Shift disambiguates the split actions cleanly. `H` = horizontal axis, `V` = vertical axis. The mnemonic is "split along the H-orizontal axis" (adding a horizontal divider = vertical split in some terminals' terminology — see note below).

**Axis naming note:** There is an industry-wide ambiguity. "Horizontal split" in tmux means a horizontal divider (two panes stacked), but in i3/hyprland it means panes side-by-side. Gunter follows i3/hyprland: `split_h` = side-by-side (divider is vertical), `split_v` = stacked (divider is horizontal). The axis refers to the direction of layout growth, not the divider orientation. This must be documented prominently.

### Ctrl+Tab / Ctrl+Shift+Tab — pane navigation

`Ctrl+Tab` and `Ctrl+Shift+Tab` are the standard browser/IDE "cycle through tabs" shortcuts (Chrome, VS Code, Firefox). This muscle memory is universal — no learning required.

**Why not Ctrl+L / Ctrl+H:** `Ctrl+L` = `clear screen` in readline (bash, zsh, fish) — used constantly. `Ctrl+H` = backspace (ASCII 0x08) in many terminals. Stealing these would break fundamental shell interactions. Removed.

**Why not Ctrl+Arrow:** `Ctrl+←/→` is word-jump in readline and most text editors. Equally problematic to steal.

**Behavior:** `Ctrl+Tab` = focus next pane (cycles through leaves in layout tree order). `Ctrl+Shift+Tab` = focus previous pane. When only one pane exists, these are no-ops. Cycle order follows the layout tree depth-first left-to-right traversal — predictable and consistent.

### Ctrl+Z — zoom and SIGTSTP

`Ctrl+Z` sends SIGTSTP (suspend process) in Unix shells. This is a hard conflict. Every programmer who uses `fg`/`bg` relies on it.

Rationale for choosing it anyway: Gunter intercepts `Ctrl+Z` only at the window/tab level, not at the pane level. If a pane is active and the user wants SIGTSTP, they should rebind `zoom_pane`. Default config should include a comment warning about this conflict. Future fix: detect if the active pane is in a shell (via OSC 133) and pass `Ctrl+Z` through when not in multiplexer mode.

**Recommended mitigation in v1:** document the conflict clearly. Add a `# WARNING: conflicts with SIGTSTP` comment in `default.toml`.

---

## 5. Pane Navigation UX

### Decision: Cycle focus via Ctrl+Tab / Ctrl+Shift+Tab (v1)

v1 navigation cycles through panes in layout tree order. `Ctrl+Tab` = next pane, `Ctrl+Shift+Tab` = previous pane. The cycle order is depth-first left-to-right traversal of the layout tree — stable and predictable regardless of split geometry.

```
                 Split(H)
                /        \
          Split(V)       Leaf C
          /      \
        Leaf A   Leaf B

Cycle order: A → B → C → A → ...

Ctrl+Tab from A → B
Ctrl+Tab from B → C
Ctrl+Tab from C → A  (wrap)
Ctrl+Shift+Tab from A → C (reverse)
```

### Why not Ctrl+L / Ctrl+H (directional)

`Ctrl+L` = readline `clear screen`. `Ctrl+H` = backspace (ASCII 0x08). Both conflict with fundamental shell interactions that fire constantly. Removed from defaults. Users who want Vim-style directional navigation can rebind in config (`next_pane = "Ctrl+Shift+L"` etc.).

### Why not numbered panes (Ctrl+1/2/3)

Numbers require mental bookkeeping. With 5 panes, a user must remember which is pane 3. Directional or cyclic is more ergonomic. Numbered pane switching remains a v2 option for power users.

### Why not mouse-only (click to focus)

Mouse focus is additive, not a replacement. Click-to-focus is always supported (see section 11). Requiring a mouse for pane navigation breaks keyboard-only workflows. Gunter is a keyboard-first terminal.

### Why Ctrl+Tab specifically

`Ctrl+Tab` is the universal "next item in tabbed UI" shortcut (browsers, VS Code, Windows Task Switcher). Zero learning curve. `Ctrl+Shift+Tab` = previous is equally universal. These are the only two pane-cycling shortcuts that have no meaningful shell conflicts.

### Leader key deferral

tmux `Ctrl+B <arrow>` for directional pane navigation is conflict-free but slow (two keystrokes). Deferred to v2 as opt-in via `keys.leader`.

### Trade-offs accepted

- Cycle order is positional (tree traversal), not spatial. If a user has panes A (left), B (top-right), C (bottom-right), `Ctrl+Tab` from A goes to B, not "the pane to my right." Spatial directional navigation (`Ctrl+Shift+L/H/J/K`) is a v2 feature.
- Wrap-around navigation (Ctrl+Tab from last pane → first). Consistent with browser tab cycling. Configurable via `keys.wrap_navigation = false` in v2.

---

## 6. Tab Model

### Decision: Flat tab list, each tab holds one Layout tree

A tab is an entry in a `Vec<Tab>`. Each `Tab` contains:
- A `Layout` tree (the pane arrangement for that tab)
- A `String` title (derived from the active pane's window title or shell CWD)
- A `Uuid` id

```rust
pub struct Tab {
    pub id:     Uuid,
    pub title:  String,
    pub layout: Layout,
}

pub struct WindowState {
    pub tabs:   Vec<Tab>,
    pub active: usize,   // index into tabs
}
```

Only the active tab renders. Inactive tabs continue running (PTYs stay alive) but do not consume GPU time.

### Why not tmux session model (sessions > windows > panes)

tmux's three-level hierarchy (session → window → pane) is powerful but has a steep learning curve. The target user of Gunter is not "power tmux user." They want simple multiplexing without the conceptual overhead of sessions. Gunter's two-level model (tab → pane) maps to browser tabs + pane splits — a model most users already know.

### Why not workspaces (hyprland model)

hyprland workspaces are virtual desktops — an application window can be on workspace 1 or workspace 3, independent of the monitor. Terminal panes don't benefit from that model. Tabs are the correct abstraction for a terminal: numbered, sequential, switchable by index.

### Tab switching keybinds (v1)

`Ctrl+Shift+T` = new tab. `Ctrl+Shift+W` = close active pane (if tab has multiple panes) or close tab (if only one pane remains). `Ctrl+1`..`Ctrl+9` = switch to tab N (v2 feature — requires adding numeric key bindings to the config schema).

Tab bar rendering: a thin bar at the top of the window. Each tab shows its index and title. Active tab highlighted with `selection` color from theme. The bar is hidden when there is only one tab (v1 default, configurable via `tabs.show_bar = "always" | "auto" | "never"`).

### Trade-offs accepted

- No session persistence in v1. Closing Gunter kills all PTYs. tmux-style session save/restore is a v2 feature (requires serializing the layout tree and PTY state).
- No tab reordering in v1. Tabs are always in creation order.

---

## 7. Zoom Mode

### Decision: Lossless single-pane zoom via layout overlay, Ctrl+Z to toggle

Zoom is not a destructive layout change. The original Layout tree is preserved in memory. Zoom works by setting a `zoom: Option<Uuid>` flag on the active Tab. When `zoom` is `Some(id)`, the renderer allocates the full window rect to that pane's session, ignoring the layout tree entirely.

```rust
pub struct Tab {
    pub id:     Uuid,
    pub title:  String,
    pub layout: Layout,
    pub zoom:   Option<Uuid>,  // None = normal, Some(id) = fullscreen that pane
}
```

When the user presses `Ctrl+Z` again, `zoom` is set back to `None` and the layout tree resumes. The zoomed pane receives a `pty.resize()` call to match the full window. On un-zoom, all panes receive `pty.resize()` to their original rects. The VT state in the zoomed pane is unaffected — it simply sees a larger terminal.

This is identical to tmux `Ctrl+B z` behavior and hyprland's `fullscreen` toggle.

### Why not replacing the layout with a single leaf

Replacing the layout would lose the original split positions. The user would have to manually re-split after un-zooming. The overlay approach is strictly superior.

### Why not a separate window

Opening a new OS window for zoom breaks the single-window multiplexer model. Focus management becomes complex. Rejected.

### Zoom indicator

When in zoom mode, the status bar (v2) or window title (v1) shows `[ZOOM]` so the user knows the layout is not visible. Pressing any pane navigation key (`Ctrl+Tab`) while zoomed un-zooms first, then navigates — this prevents confusion about "why isn't pane cycling working."

### Trade-offs accepted

- Zoomed pane receives a resize SIGWINCH. Apps that react poorly to rapid resize/un-resize (rare) may see a brief redraw artifact. This is the same trade-off tmux accepts.

---

## 8. Font Fallback Chain

### Decision: Ordered fallback list in config, `wezterm-font` handles shaping

```toml
[font]
family   = "ComicShannsMono Nerd Font Mono"
fallback = ["Noto Sans CJK SC", "Segoe UI Emoji", "Symbols Nerd Font Mono"]
size     = 14.0
line_height = 1.2
```

`wezterm-font` implements font fallback natively: for each codepoint, it walks the font list until a font covers it, then rasterizes from that font. Gunter delegates entirely to `wezterm-font` for this logic.

### Fallback chain rationale

1. **ComicShannsMono Nerd Font Mono** — primary. Covers ASCII, Latin, and Nerd Font private-use area (U+E000–U+F8FF, U+100000–U+10FFFD). Powerline symbols, devicons, git branch icons are here.
2. **Noto Sans CJK SC** — Chinese/Japanese/Korean. Noto is designed for metric compatibility with Latin fonts at the same point size. This minimizes line height jumps when CJK characters appear inline.
3. **Segoe UI Emoji** — Windows emoji. Color emoji (CBDT/CBLC or COLR format). On Linux use `Noto Color Emoji` instead. The fallback list should differ by OS — v2 feature; v1 hardcodes the Windows-friendly list given the WSL2 target.
4. **Symbols Nerd Font Mono** — safety net for any Nerd Font symbols not in the primary font.

### Private use area handling

Unicode U+E000–U+F8FF is the private use area. Nerd Fonts place icons here. The primary font (ComicShannsMono Nerd Font Mono) already includes these. The `wezterm-font` atlas will rasterize them at the same cell size as ASCII glyphs. No special handling needed.

### Missing glyph fallback (tofu)

When no font in the chain covers a codepoint, `wezterm-font` renders a replacement glyph (empty box or `?`). Gunter logs a warning (once per codepoint) to help users identify missing font coverage.

### Why not fontconfig / system font matching

fontconfig is Linux-only and adds a system dependency. `wezterm-font` already abstracts font discovery for Windows and Linux. Gunter uses `wezterm-font`'s API and avoids a direct fontconfig dependency.

### Trade-offs accepted

- Font metrics (advance width, ascent, descent) are taken from the primary font. Fallback fonts that have different metrics at the same point size can cause misalignment. Mitigation: Noto is designed to match metrics. Emoji fonts are always single-cell wide by Gunter convention (double-width emoji are v2).
- Font loading is synchronous at startup. A config-specified font that does not exist on the system causes a startup error with a helpful message, not a crash.

---

## 9. Theme System

### Decision: Named themes as separate TOML files in `assets/themes/`, loaded by name in config

```
assets/themes/
  atom-one-dark.toml
  gruvbox-dark.toml     (v2)
  catppuccin-mocha.toml (v2)
  solarized-dark.toml   (v2)
```

The config references a theme by name:

```toml
[theme]
name = "atom-one-dark"
```

Gunter searches for the theme file at startup in this order:
1. `~/.config/gunter/themes/<name>.toml` (user override)
2. `assets/themes/<name>.toml` (built-in, embedded via `include_str!` at compile time)

A user can create `~/.config/gunter/themes/my-theme.toml` and set `name = "my-theme"` to use it. Built-in themes are compiled into the binary — no runtime asset path issues.

### Theme file schema

```toml
# assets/themes/atom-one-dark.toml
[colors]
background  = "#282c34"
foreground  = "#abb2bf"
cursor      = "#528bff"
selection   = "#3e4451"

# Standard 16 ANSI colors
black        = "#3f4451"
red          = "#e06c75"
# ... (full 16)

# Optional: bright variants
bright_black = "#4f5666"
# ...
```

All color values are hex RGB strings. Alpha channel is supported as 8-digit hex (`#rrggbbaa`) for semi-transparent backgrounds (v2 feature; v1 ignores alpha).

### Why not inline colors in config (Alacritty model)

Alacritty puts colors directly in `alacritty.yml`. This works but makes it impossible to share theme files between users without copy-pasting a large block. Named theme files are more composable. Hot reload of a named theme is also simpler: just reload the theme file, not the entire config.

### Why not a theme registry / download system

Out of scope for v1. Users who want more themes use the user override directory. A `gunter theme install <url>` command is a v2 feature.

### Hot reload for themes

When `theme.name` changes in the config, Gunter reloads the theme file and updates the wgpu color uniforms in the next render frame. When the theme file itself changes (user is editing their custom theme), the `notify` watcher must also watch `~/.config/gunter/themes/` — this is a v2 refinement. v1 only watches the main config file.

### Trade-offs accepted

- Built-in themes are compiled into the binary via `include_str!`. Adding a theme requires a recompile. This is acceptable for v1 where only one theme ships. v2 ships themes as assets alongside the binary or scans `XDG_DATA_DIRS`.

---

## 10. Scrollback UX

### Decision: Vim-key scrollback mode entered via keyboard shortcut, mouse scroll always works

**Mouse scroll** always scrolls the viewport without entering a special mode. This is the no-ceremony path.

**Scrollback mode** (`Ctrl+Shift+S` default, v2 — not in v1) is a modal state where:
- Arrow keys / `j`/`k` scroll line by line
- `Ctrl+U`/`Ctrl+D` scroll half-page
- `/` enters search mode
- `y` copies current selection to clipboard
- `q` or `Esc` exits

v1 ships with mouse scroll only. Keyboard scrollback mode is v2.

### Scrollback buffer implementation

The `Grid` struct uses a `VecDeque<Vec<Cell>>` for scrollback. Max lines is configurable (`scrollback.lines = 5000`). When the buffer is full, the oldest line is dropped from the front. This is O(1) per push/pop.

### Search in scrollback (v2)

Incremental search (`/pattern`) highlights all matches and jumps to the nearest one. Search state is ephemeral — it does not persist after exiting scrollback mode.

### Copy from scrollback

In v1, mouse selection always works: click-drag selects text, release copies to clipboard (see section 11). In v2 scrollback mode, `y` copies the visual selection.

### Why not tmux-style copy mode only

tmux requires entering copy mode to scroll at all. Mouse scroll is a quality-of-life improvement that modern terminals (kitty, WezTerm, Alacritty) all support. Gunter ships mouse scroll in v1 because it requires no extra modal UI.

### Trade-offs accepted

- `scrollback.lines = 5000` default is conservative. Memory cost: 5000 lines × 220 cols × ~40 bytes per Cell ≈ 44 MB worst case. Practical usage is much lower (most lines are shorter). Users with high memory pressure should reduce this.

---

## 11. Copy/Paste

### Decision: OSC 52 for clipboard writes, system clipboard on paste, bracketed paste always on

**Copy (writing to clipboard):**
Gunter uses OSC 52 escape sequences (`\e]52;c;<base64>\a`) to write to the system clipboard. This works in both native Windows (where the system clipboard is the target) and WSL2 (OSC 52 is forwarded to the Windows clipboard via the ConPTY layer). Applications inside the terminal (Neovim with `set clipboard=unnamedplus`) can write to the clipboard using OSC 52 and Gunter passes it through to the OS clipboard.

Mouse selection: when the user releases a mouse selection, Gunter copies the selected text to the OS clipboard via the platform clipboard API (`arboard` crate or `winit`'s clipboard support). No manual `Ctrl+C` needed.

**Paste (reading from clipboard):**
`Ctrl+Shift+V` pastes the OS clipboard contents. The text is wrapped in bracketed paste sequences (`\e[?2004h` enable, `\e[200~<text>\e[201~` on paste) so applications that support bracketed paste can distinguish pasted text from typed text.

`Ctrl+C` is NOT intercepted for copy. It is passed through to the PTY as SIGINT. This matches every other terminal's behavior and avoids breaking shell workflows.

### Why not X11 PRIMARY selection

X11 PRIMARY (middle-click paste) is a Linux-native concept. Gunter runs on Windows/WSL2. PRIMARY selection is not available from the Windows side of WSL2. It is a future Linux-native build feature.

### Why not Ctrl+C / Ctrl+V (Windows model)

Intercepting `Ctrl+C` breaks SIGINT. The established convention for terminals is `Ctrl+Shift+C`/`Ctrl+Shift+V`. Gunter follows this convention.

### Bracketed paste

Bracketed paste MUST be on by default. Without it, pasting multi-line text into a shell prompt runs each line as a command. This is a security and correctness issue. Gunter enables bracketed paste mode at PTY startup and after each mode reset.

### WSL2 clipboard specifics

In WSL2, the PTY is a Windows ConPTY. OSC 52 sequences in the byte stream are processed by Windows Terminal / ConPTY and write to the Windows clipboard. Gunter should also handle OSC 52 directly (intercepting it before passing to the PTY) so clipboard operations work even when the underlying PTY does not forward OSC 52. Both paths (Gunter-handled and PTY-forwarded) should result in clipboard writes.

### Trade-offs accepted

- Clipboard access requires OS clipboard API calls which may have small latency. For the paste path, Gunter reads the clipboard synchronously in the event loop tick. For buffers >1 MB, this may cause a brief frame drop (v2: async paste).

---

## 12. Mouse Support

### Decision: Click to focus, drag to select, scroll to scroll, resize via border drag; forwarding is opt-in per pane

**Gunter-handled mouse events:**
- **Click in pane** → focus that pane (no click forwarded to app unless pane was already focused)
- **Click-drag in focused pane** → text selection; release copies to clipboard
- **Scroll wheel in pane** → scroll viewport (no PTY event sent)
- **Click-drag on pane border** → resize pane (adjusts `ratio` in Layout::Split node)
- **Click on tab bar** → switch to that tab

**Forwarded to PTY (when app requests mouse events):**
When the PTY application sends `\e[?1000h` (X10 mouse), `\e[?1002h` (button events), or `\e[?1003h` (all events), Gunter switches the pane to mouse-forwarding mode. In this mode:
- Mouse clicks are encoded as VT mouse events and sent to the PTY
- Text selection is disabled (or requires holding `Shift`)
- Scroll is forwarded as arrow key equivalents

**Shift modifier override:** holding `Shift` during mouse events always uses the Gunter-handled path (selection/scroll), even when the app has requested mouse events. This matches kitty behavior and allows the user to select text from a vim session.

### Pane border hit detection

Pane borders are 2px wide (configurable). The click target is expanded to 4px for ergonomics. On hover, the border highlights with the `selection` color to indicate draggability (v2 — v1 has no hover state).

### Why not always forward mouse events

Apps that use raw mouse events (Neovim, htop, fzf) require forwarding. Apps that don't use it (shells, most CLI tools) benefit from Gunter-level selection. The mode must be switchable per pane per VT escape sequence.

### Trade-offs accepted

- In mouse-forward mode, text selection requires `Shift+drag`. This is the universal convention (kitty, WezTerm, Alacritty all do this). Users who forget `Shift` may be confused initially.

---

## 13. Performance UX Targets

### Decision: Target <5ms input latency, 60fps render, <10ms keystroke-to-display

**Measurements that matter:**

| Metric | Target | Kitty baseline | Alacritty baseline |
|---|---|---|---|
| Keystroke to display | <10ms | ~8ms | ~6ms |
| Input latency (key down to PTY) | <1ms | ~1ms | ~0.5ms |
| Throughput (cat large file) | >1 GB/s PTY read | ~800 MB/s | ~600 MB/s |
| Frame time at 60fps | <16.6ms | ~14ms | ~12ms |
| Frame time at 120fps (v2) | <8.3ms | not default | not default |

**How to achieve these:**

1. **Dirty-only cell upload:** only upload changed cells to the GPU vertex buffer. A 220×50 grid (11,000 cells) at 40 bytes each is 440 KB. Uploading the full grid every frame at 120fps = 52 MB/s GPU upload bandwidth — wasteful. Per-cell dirty flags reduce this to a few KB per frame in steady state.

2. **Tokio task separation:** PTY read, VTE parse, and grid update run in a separate tokio task. The winit event loop only reads the committed grid state. No lock contention on the hot path. A `RwLock<Grid>` with writer being the parse task and reader being the render task is the correct primitive.

3. **Double buffering:** the renderer maintains a "last rendered frame" grid snapshot. On render, diff the current grid against the snapshot, upload only deltas.

4. **VSync strategy:** Gunter renders at the display's native refresh rate. wgpu `PresentMode::Fifo` (VSync) is the default. `PresentMode::Mailbox` (low-latency, allows tearing) is configurable via `renderer.present_mode = "vsync" | "mailbox"`.

### Why not target 240fps

overkill for a terminal. No visible benefit over 120fps for text rendering. 60fps is the v1 target; 120fps is the v2 stretch goal.

### Benchmarking approach

Use `vtebench` (the standard VTE throughput benchmark) and a custom input latency tool that measures time from OS key event to pixel change. Both should be part of CI as regression tests (v2).

### Trade-offs accepted

- `PresentMode::Fifo` (60fps VSync) adds up to 16ms additional latency on key presses. For a 60Hz display this is imperceptible. For a 144Hz display, `Mailbox` mode is recommended. v1 defaults to Fifo for simplicity.

---

## 14. Startup UX

### Decision: Auto-detect shell, generate default config on first run, no migration wizard

**Shell detection order:**
1. `SHELL` environment variable (POSIX standard)
2. `~/.config/gunter/config.toml` `shell.program` (explicit override)
3. `/etc/passwd` entry for current user
4. Fallback: `wsl.exe` on Windows, `/bin/sh` on Linux/macOS

On first run (no config file exists), Gunter:
1. Detects the shell
2. Writes a commented `~/.config/gunter/config.toml` with all defaults and `# OPTIONAL:` markers for non-required settings
3. Opens a pane with the detected shell

The generated config includes a `config_version = 1` key for future migration.

**Config migration (v2):** when `config_version` in the file is less than the current version, Gunter runs a migration function that updates the schema (renaming keys, adding new required fields with defaults) and writes the new config back. v1 has no migration because there is only one version.

### First run experience

No onboarding wizard. Gunter opens with a functional terminal immediately. The generated config is the documentation. Comments in `default.toml` explain every option.

### Why not a TUI config wizard

Config wizards are a maintenance burden and become stale. The config file with comments is self-documenting. The target user is a developer who can read TOML.

### Trade-offs accepted

- If `SHELL` is unset and `/etc/passwd` is unavailable (some container environments), the fallback to `wsl.exe` is surprising on Linux. A clear error message ("could not detect shell, defaulting to wsl.exe, set SHELL or configure shell.program") mitigates this.

---

## 15. Hyprland Inspiration

### What to port for v1

| Hyprland pattern | Gunter v1 implementation |
|---|---|
| Directional focus (`movefocus l/r`) | `Ctrl+Tab`/`Ctrl+Shift+Tab` cyclic pane navigation (directional is v2) |
| Binary split tree (i3-style) | `Layout::Split` enum, same as hyprland's internal tree |
| Fullscreen toggle | Zoom mode (`Ctrl+Z`) — overlay, not destructive |
| Active window border highlight | Active pane border rendered in `cursor` color (1px) |
| Snappy no-animation default | 0ms resize/split animation in v1 — instant |

### What is NOT in v1

| Hyprland feature | Reason deferred |
|---|---|
| Workspaces (virtual desktops) | Tabs cover the same UX need for a terminal |
| Animations (slide, fade, zoom) | Adds rendering complexity; optional luxury |
| Scratchpad (hidden floating pane) | Useful but complex; v2 |
| Per-monitor workspaces | Single-window terminal; not applicable |
| Window rules (match by class) | PTY sessions are too uniform for this to matter in v1 |
| `hyprctl dispatch` IPC | Gunter socket server (`gunter attach`) covers the IPC need |
| Rounded corners | Purely cosmetic; v2 (`renderer.border_radius = 8`) |
| Blur / transparency | wgpu compositing complexity; v2 |

### Active pane border

The active pane is highlighted with a 1px border in the theme's `cursor` color. Inactive pane borders are rendered in `bright_black`. This is the only visual affordance distinguishing the focused pane — no glow, no shadow in v1.

### Snappiness

hyprland's defining characteristic is that UI actions feel instantaneous. For Gunter this means:
- Split: new pane appears in the same render frame the key was pressed (no animation)
- Tab switch: 0ms (only the active tab renders; switching is just a Vec index change)
- Config hot reload: applied within the next render frame after the debounce window

No animations are planned for v1. If animations are added in v2, they must be opt-in and default-off.

---

## 16. v1 Scope Summary

The following table clarifies what ships in v1 vs what is planned for v2.

| Feature | v1 | v2 |
|---|---|---|
| TOML config | Yes | — |
| Hot reload (theme, keys, scroll) | Yes | — |
| Hot reload (font family) | No | Yes |
| Atom One Dark theme | Yes | — |
| Additional built-in themes | No | Yes |
| User theme directory | Partial (file loads, no watcher) | Full watcher |
| Static enum keybindings | Yes | — |
| Leader key | No | Yes |
| Chord bindings | No | Yes |
| Tab model | Yes | — |
| Pane splits (H + V) | Yes | — |
| Directional focus L/R | Yes | — |
| Directional focus U/D | No | Yes |
| Zoom mode | Yes | — |
| Mouse scroll | Yes | — |
| Mouse resize pane border | Yes | — |
| Keyboard scrollback mode | No | Yes |
| Scrollback search | No | Yes |
| OSC 52 clipboard | Yes | — |
| Bracketed paste | Yes | — |
| Shell integration (OSC 133) | No | Yes |
| `Ctrl+W` passthrough for kill-word | No | Yes (shell integration) |
| Session persistence | No | Yes |
| Socket server / `gunter attach` | Phase 4 | — |
| Animations | No | Opt-in |
| Scratchpad | No | Yes |
| Rounded corners | No | Yes |
| Config migration | No | Yes |
| Tab reordering | No | Yes |
| Tab switching by number | No | Yes |
| 120fps mode | No | Yes |
