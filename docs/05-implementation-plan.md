# Gunter — Session Planning Reference

See SPEC.md for phase goals, steps, exit conditions, and complexity ratings.

---

## Versioning Strategy (phases → semver)

| SPEC Phase | Version | Rationale |
|---|---|---|
| Phase 1 | v0.1.0 | Headless — no GPU dependency |
| Phase 2 | v0.2.0 + v0.3.0 | GPU renderer (v0.2) and keyboard interaction (v0.3) split because GPU alone is not demoable as interactive |
| Phase 3 | v0.4.0 | Multiplexer — requires proven renderer + input |
| Phase 4 | v0.5.0 | Socket server is opt-in extensibility; does not block Phase 3 |
| Phase 5 | v0.6.0 | Config wires all prior hardcoded values; touches every subsystem |
| — | v1.0.0 | Production hardening, packaging, perf targets — not a new phase |

---

## Sequencing Rationale

- Phase 1 before Phase 2: a broken Grid draws wrong at 60fps. Prove cell state correctness headless first.
- Phase 2 split into v0.2 (render) + v0.3 (input): v0.2 exit is "see text"; v0.3 exit is "usable terminal". Natural dogfood gate between them.
- Phase 3 before Phase 4: `WindowState` / layout tree introduced in Phase 3 is what the socket server wraps. Invert = build on unstable API.
- Phase 5 last: config hot-reload modifies every subsystem (atlas, theme uniform, InputMap). Stable interfaces required first.

---

## Cross-Phase Dependencies

- v0.3.0 introduces `RwLock<Grid>` shared between VTE task (writer) and render loop (reader). If `RwLock` contention appears under load, double-buffer scheme (back buffer + per-frame swap) is the resolution — defer unless profiling shows it needed.
- v0.4.0 replaces the single `Session` in `app.rs` with `WindowState`. Anything in v0.3.0 that holds a direct `Session` reference must be refactored at v0.4.0 boundary.
- v0.5.0 introduces `ShellConfig` stub; v0.6.0 replaces it with full TOML-backed config. Stub must match the field shape of the final type.
- `wezterm-font` vendoring (v0.2.0) is the highest project risk. Fallback: `fontdue` for ASCII-only, defer Nerd Font to v0.6.0. Decide at start of v0.2.0 session; do not discover mid-session.

---

## Session Entry Points

**Before starting any phase — check:**
```
cargo check          # workspace compiles clean
cargo test           # prior phase unit tests pass
bd ready             # no blocking issues open
```

**v0.1.0:** No prior state needed. WSL2 + MSVC toolchain must be available on machine. Confirm `wsl.exe` in PATH before writing code.

**v0.2.0:** `wezterm-font` vendoring decision must be made first (see risk above). Have `ComicShannsMono Nerd Font Mono` installed. Confirm `wgpu` DX12 works: `wgpu::Instance::enumerate_adapters(Backends::DX12)` returns at least one adapter.

**v0.3.0:** v0.2.0 window opens and renders static text. `vtebench` available for throughput smoke test at session end.

**v0.4.0:** Single-pane terminal is daily-drivable (v0.3.0 complete). Refactor `app.rs` session reference to `WindowState` as first commit of the session.

**v0.5.0:** Named pipe / Unix socket behavior tested manually with `nc`/`socat` before writing relay code. Windows ACL on named pipes: verify a second process can connect without elevated rights.

**v0.6.0:** All prior hardcoded values (Atom One Dark hex, font size, keybinds) catalogued before starting — these are what the config replaces. Font family hot-reload is out of scope; log a restart message, do not attempt async font swap.

---

## Known Risks Not in SPEC

- `wgpu` 0.20+ changed `Surface::get_current_texture` API. Pin version in workspace `Cargo.toml` at project start.
- `portable-pty` ConPTY behavior varies across Windows versions. Test on Win10 1809 minimum.
- `arboard` (clipboard) requires COM initialization on Windows main thread. Call `CoInitializeEx` early in `main()` before any clipboard use.
- Binary size: `wezterm-font` is the largest contributor. Run `cargo bloat --release` before v1.0.0 packaging. Contingency: `cosmic-text`.
- Tab bar adds one row height; triggers `pty.resize()` on all sessions when first tab opens. Expected; not a bug.
