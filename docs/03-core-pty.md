# Core / PTY / Terminal-Emulation — Implementation Notes

> See SPEC.md for Cell/Grid/Session/Layout types and Phase 1 skeleton.

---

## PTY reader thread

`portable-pty`'s `try_clone_reader()` is a **blocking** `Read`. Never call it inside a tokio task — it stalls a worker thread. One dedicated OS thread per session, tight `loop { read; blocking_send }`. On Windows/ConPTY, the first `read` after `openpty` may block until the child writes something; this is normal.

Shutdown: set `Arc<AtomicBool>` before dropping the PTY master fd. The blocking `read()` unblocks with an error when the master fd closes; check the flag to distinguish clean shutdown from real I/O error.

Child exit: `read()` returns `Ok(0)` (POSIX) or an error (Windows ConPTY). Send `SessionEvent::ChildExited(id)` through a separate event channel to the main tokio loop. Never leave zombie sessions in the map — they hold PTY fds and threads.

## Channel backpressure

PTY→VTE channel: bounded at **32 × 4096 B = 128 KB** in flight. If the VTE processor falls behind (e.g. `cat bigfile`), the reader thread parks via `blocking_send`. Do not make it unbounded — that hides backpressure and balloons RAM.

Input→PTY writer: bounded at 64. Use `try_send` from the winit event loop — drop keystrokes rather than block the event loop. At 64-item capacity this should never trigger in practice.

## Dirty flag lifecycle

Set `dirty[idx] = true` at every cell write inside `GridPerformer`. Set `all_dirty = true` on `grid.resize()`, `enter_alt_screen()`, `exit_alt_screen()`. The renderer reads `all_dirty` first — if set, skip per-cell scan. **Clear dirty flags in the renderer after upload**, not in `GridPerformer`. Clearing them before the GPU draw would race with a concurrent PTY burst producing new mutations.

## vte::Perform → Grid mutations

`vte` calls `Perform` methods **synchronously inside `parser.advance()`**. All Grid state changes happen there; there is no deferred queue.

Key gotchas:

- **SGR colon subparams** (`38:2:r:g:b`): required by neovim/kitty. `vte::Params` supports sub-parameters; handle both `;`-separated and `:`-separated forms from day one or face "wrong color in neovim" issues.
- **LF at scroll bottom**: only scroll when `cursor.row == scroll_bottom` (DECSTBM region). Scrolling outside the scroll region is a common bug — vim uses a non-default scroll region heavily.
- **Auto-wrap flag**: some apps set `DECAWM off` (CSI ? 7 l). Track it; if off, clamping cursor at col = cols-1 instead of wrapping is required for correct rendering of right-margin content.
- **DECSC/DECRC** (`ESC 7` / `ESC 8`): save/restore cursor + SGR state. Missing this breaks `vim` suspend/resume and many prompts.
- **Synchronized output** (`CSI ? 2026 h/l`): defer dirty-flush to the renderer until the `l` sequence arrives. Without it, fast output (terminal animations, progress bars) tears.

## Alternate screen

`enter_alt_screen`: save primary cursor, allocate `alt_cells` (blank, same dims), set `using_alt_screen = true`, set `all_dirty`. Alt screen has **no scrollback**. `exit_alt_screen`: drop `alt_cells`, restore cursor, set `all_dirty`. Renderer always reads `grid.active_cells()` — no renderer changes needed on switch.

## Layout reflow specifics

`reflow` is called on every window resize and on every split/close. Order matters: call `grid.resize(cols, rows)` **before** `pty.resize(PtySize{...})`. Resizing the Grid first ensures the cell buffer is the right size before the child process (which may immediately write SIGWINCH-triggered redraws) produces output.

On POSIX, `pty.resize()` sends `SIGWINCH` to the child. On Windows, ConPTY handles resize internally — no signal. `ratio: f32` can drift; clamp to `[0.05, 0.95]` to prevent zero-height/width panes.

## Windows ConPTY / WSL quirks

- ConPTY requires Windows 10 build 17763+. WSL2 requires 1903+. Safe floor: 1903.
- `portable-pty`'s ConPTY path does not support `SIGWINCH` — resize is via `ResizePseudoConsole` internally; `pty.resize()` wraps this correctly.
- WSL pipe behavior: `wsl.exe` is a proxy process; the real shell runs inside the VM. PTY output crosses a Hyper-V socket. Expect occasional bursts with inter-packet gaps — do not interpret a gap as EOF. Only `Ok(0)` or a real error signals EOF.
- Named pipe paths on Windows: `\\.\pipe\gunter-{uuid}`. Use `tokio::net::windows::named_pipe::ServerOptions` (tokio 1.15+). The `\\.\pipe\` prefix is mandatory; omitting it silently creates a file.
