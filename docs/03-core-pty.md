# Gunter Core / PTY / Terminal-Emulation Subsystem — Design Document

> Crates: `gunter-core`, `gunter-term`
> Last updated: 2026-05-25

---

## Summary

This document covers every significant design decision in the two crates that form the foundation of Gunter:

- **`gunter-core`** — PTY allocation, session lifecycle, layout tree, socket server, input routing
- **`gunter-term`** — VT/ANSI parser integration, `Grid` data structure, cell representation, scrollback buffer, terminal state machine

Everything above this layer (rendering, input mapping, window management) depends on these decisions being correct. A wrong grid state draws wrong; a wrong PTY architecture leaks file descriptors, orphans child processes, or stalls on Windows. Get these right first.

The 12 topics below are ordered from most foundational (PTY backend) to most application-level (terminal state machine). Each section states the decision, explains why alternatives were rejected, and lists trade-offs accepted.

---

## 1. PTY Backend — `portable-pty`

### Decision

Use `portable-pty` (from the WezTerm project) as the sole PTY abstraction. Call `portable_pty::native_pty_system()` to get a `PtySystem`, then `pty_system.openpty(PtySize { rows, cols, pixel_width, pixel_height })` to get a `PtyPair`. Spawn child processes through `PtyPair::slave.spawn_command(CommandBuilder)`.

```rust
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

let pty_system = native_pty_system();
let pair = pty_system.openpty(PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 0,
    pixel_height: 0,
})?;
let cmd = CommandBuilder::new("wsl.exe");
let child = pair.slave.spawn_command(cmd)?;
let reader = pair.master.try_clone_reader()?;
let writer = pair.master.take_writer()?;
```

### Why not the alternatives

**wezterm-mux**

`wezterm-mux` is WezTerm's higher-level multiplexing layer, built on top of `portable-pty`. It includes session serialization, domain abstraction, pane IDs, and a wire protocol. Using `wezterm-mux` would mean adopting WezTerm's entire session model, including assumptions about GUI domains, pane tabs, and the mux server protocol. Gunter has its own session and layout model; importing `wezterm-mux` would create a deep structural conflict and pull in large dependency trees. `portable-pty` is the correct abstraction layer — it provides PTY primitives without imposing a multiplexer architecture.

**Custom ConPTY bindings**

Windows ConPTY (`CreatePseudoConsole`, `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`) can be called directly via the `windows` crate. Problems: (a) ConPTY is Windows-only — any POSIX PTY (Linux, macOS) would need a completely separate code path; (b) `portable-pty` already wraps ConPTY correctly and has been battle-tested in WezTerm, which runs in thousands of environments. Reimplementing this is unjustified work. The only valid reason would be if `portable-pty`'s ConPTY implementation had a blocking bug; that is not the case.

**alacritty-terminal's PTY module**

Alacritty has its own PTY code (`alacritty_terminal::tty`). It is not published as a standalone crate and is deeply coupled to Alacritty's own `Config` and event model. Using it would require forking or vendoring Alacritty's codebase. Not practical. `portable-pty` is the correct choice precisely because it is a standalone, purpose-built crate.

**Raw POSIX `openpty`/`forkpty` via libc**

`libc::openpty` works on POSIX systems but is not available on Windows at all. A raw POSIX implementation would need an entirely separate Windows path via ConPTY. `portable-pty` is exactly the cross-platform abstraction layer that makes raw POSIX calls unnecessary.

### Trade-offs accepted

- `portable-pty` is developed in the WezTerm monorepo, so releases track WezTerm's cadence, not Gunter's. Pin to a specific version in `Cargo.toml` and upgrade deliberately.
- On Windows, `portable-pty`'s ConPTY path requires Windows 10 version 1809 (build 17763) or later. Earlier versions do not have ConPTY. This is an acceptable floor given that WSL2 also requires Windows 10 1903+.
- `portable-pty`'s reader (`try_clone_reader`) returns a blocking `Read` impl. It must be polled on a dedicated OS thread, not inside a tokio async task (see Section 6).

---

## 2. VT/ANSI Parser — `vte`

### Decision

Use the `vte` crate as the sole VT/ANSI parser. Instantiate `vte::Parser` per session, call `parser.advance(&mut performer, byte)` for each byte read from the PTY. Implement `vte::Perform` on a `GridPerformer` struct that holds a mutable reference to the session's `Grid`.

```rust
use vte::{Parser, Perform};

pub struct GridPerformer<'a> {
    pub grid: &'a mut Grid,
    pub title_tx: Option<&'a mpsc::Sender<String>>, // OSC 0/2 title changes
}

impl<'a> Perform for GridPerformer<'a> {
    fn print(&mut self, c: char) { /* write cell */ }
    fn execute(&mut self, byte: u8) { /* C0 controls: LF, CR, BS, BEL, TAB */ }
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) { /* cursor, erase, SGR */ }
    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) { /* title, color palette */ }
    fn hook(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) { /* DCS entry */ }
    fn put(&mut self, byte: u8) { /* DCS data bytes */ }
    fn unhook(&mut self) { /* DCS exit */ }
    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) { /* ESC sequences */ }
}
```

### Why not the alternatives

**termwiz**

`termwiz` is WezTerm's terminal widget crate. It includes a parser, an `Action` enum representing parsed escape sequences, a `Terminal` trait, and a surface abstraction. It is a significantly heavier dependency than `vte` — it includes SSH support, multiplexer protocol code, and widget rendering primitives that Gunter has no use for. More importantly, `termwiz` allocates `Action` enum variants on the heap per parsed sequence, which is exactly the allocation pattern `vte` avoids. At PTY read rates of 100–500 KB/s (a fast `cat` of a large file), allocation-free parsing meaningfully reduces GC pressure even in a language with deterministic allocation like Rust, because it reduces allocator contention between the PTY reader thread and the tokio runtime.

**alacritty_terminal's parser**

Like the PTY code, Alacritty's parser is not a standalone published crate. It is also built on top of `vte` internally. Using Alacritty's terminal would mean vendoring Alacritty's entire grid model, which conflicts with Gunter's own `Grid` design. The correct approach is to use `vte` directly and implement `Perform` in Gunter's own code.

**Custom parser**

A custom VT/ANSI parser would need to correctly implement: ANSI X3.64 (C0, C1 controls), ECMA-48 (CSI sequences), DEC private sequences (DECSC, DECRC, DECSTBM, mouse modes), OSC (title, color change), DCS (sixel, DECRQSS), XTerm extensions. This is approximately 3,000–5,000 lines of state machine code with extensive edge cases. The `vte` crate implements the correct state machine from Paul Flo Williams' `vt_machine` specification (https://vt100.net/emu/dec_ansi_parser). This is not worth reimplementing.

**kitty's parser approach**

kitty implements its own parser in C (via Python extension). This is inapplicable to a Rust project. Mentioned for completeness.

### Zero-alloc vs performance trade-offs

`vte` is zero-allocation: `Params` and `intermediates` passed to callbacks are slices into an internal stack-allocated buffer. The parser struct itself is 4 bytes (state byte + intermediate buffer). This means calling `parser.advance()` in a tight loop with a 4096-byte buffer is cache-friendly and allocation-free. The alternative (`termwiz` style: allocate an `Action` per sequence) would generate one heap allocation per escape sequence. In a `cat bigfile.txt` scenario with thousands of SGR color sequences per second, this adds meaningful allocator pressure.

Trade-off accepted: `vte`'s zero-alloc design means all processing happens in the `Perform` callbacks, synchronously. There is no intermediate `Vec<Action>` to inspect after the fact. All grid mutations happen immediately during `advance()`. This is correct behavior.

---

## 3. Grid Data Structure — Flat `Vec<Cell>`

### Decision

Store the terminal grid as a flat `Vec<Cell>` of length `cols * rows`, row-major. Cell at `(col, row)` is at index `row * cols + col`.

```rust
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,          // len = cols * rows
    pub dirty: Vec<bool>,          // len = cols * rows
    pub all_dirty: bool,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub cursor: (u16, u16),
    pub scroll_top: u16,           // top of scroll region (DECSTBM)
    pub scroll_bottom: u16,        // bottom of scroll region
}
```

### Why not the alternatives

**`Vec<Vec<Cell>>` (vec of row vecs)**

Each row is an independent heap allocation. For a 220-column grid, this is one pointer dereference per row access — the inner `Vec` must be followed to reach cell data. More critically, rows are not contiguous in memory. A scroll operation (shift rows up by one) on `Vec<Vec<Cell>>` is O(rows) pointer copies plus one allocation/deallocation for the evicted row. With a flat `Vec<Cell>`, a scroll is a `copy_within` (memmove) + clearing the last row in-place — one cache-friendly contiguous operation.

The only advantage of `Vec<Vec<Cell>>` is that variable-width rows are possible (for sparse representation). Terminal grids are not sparse: every cell in the grid is always addressable, whether it contains content or a space. Variable-width rows add complexity with no benefit here.

**Arena allocation**

A bump allocator (e.g. `bumpalo`) allocates cells in a linear arena. This improves allocation speed but does not improve cache locality beyond what a flat `Vec<Cell>` already provides — a `Vec<Cell>` is already a contiguous arena. Arena allocation's main benefit is fast bulk deallocation, which is useful for short-lived trees (e.g. AST nodes). The terminal grid lives for the session lifetime; bulk deallocation is not a concern. No benefit over `Vec<Cell>`.

**`smallvec` or `tinyvec` per row**

Avoids heap allocation for short rows. Terminal rows are always full-width (220 cells minimum). Short-row optimization would never trigger. Not applicable.

**ECS-style component arrays**

Store `ch`, `fg`, `bg`, and `flags` in separate parallel arrays for SIMD-friendliness. This is only beneficial if operations frequently scan a single attribute across all cells (e.g. "find all cells with bold flag set"). The primary access pattern is "read all attributes for cell `(col, row)` to build a `GlyphInstance`" — this access pattern is a struct-of-arrays pessimization. Keep AoS (Array of Structs).

### Cache locality analysis

A `Cell` is currently:
```
char (4 bytes) + Color (4 bytes fg) + Color (4 bytes bg) + CellFlags (1 byte) + 3 padding
= 16 bytes per Cell (with alignment)
```

A 220×50 grid = 11,000 cells × 16 bytes = 176 KB. L2 cache on modern CPUs is typically 256 KB–1 MB. The entire live grid fits in L2. Dirty-flag scan (11,000 bools = 11 KB) fits in L1. This is the ideal cache profile for the renderer's per-frame walk.

Scroll operation via `copy_within`:
```rust
// Scroll up one line within scroll region [top, bottom]
let top_idx = top * cols as usize;
let bot_idx = (bottom + 1) * cols as usize;
// Shift cells[top_idx .. bot_idx] up by one row
self.cells.copy_within(
    top_idx + cols as usize .. bot_idx,
    top_idx,
);
// Clear the last row
let last_row_start = (bottom as usize) * cols as usize;
for cell in &mut self.cells[last_row_start .. last_row_start + cols as usize] {
    *cell = Cell::default();
}
```

`copy_within` compiles to `memmove`. On a 220×50 grid, scrolling 50 rows up is a 176 KB memmove — well within L2, typically completing in under 50 µs.

### Trade-offs accepted

- Resize requires `Vec::resize`, which is O(new_size). On resize, `all_dirty = true` is set anyway, so the cost is amortized. No special handling needed.
- Flat indexing means column/row bounds must be checked at write sites. Add a debug-mode `assert!` in the cell accessor; remove in release. Do not add runtime bounds checks on the hot path.

---

## 4. Scrollback Buffer — `VecDeque<Vec<Cell>>`

### Decision

Store scrollback as `VecDeque<Vec<Cell>>` where each element is one complete row of cells. Maximum size is configurable (`scrollback.lines` in config, default 5000). When the grid scrolls up past the scroll region, push the evicted row to the back of the `VecDeque`. When the deque exceeds `max_lines`, `pop_front` discards the oldest row.

```rust
pub struct Grid {
    pub scrollback: VecDeque<Vec<Cell>>,
    pub max_scrollback: usize,   // from config
}

impl Grid {
    pub fn push_scrollback_line(&mut self, row: Vec<Cell>) {
        if self.scrollback.len() >= self.max_scrollback {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(row);
    }
}
```

### Why not the alternatives

**Flat `Vec<Cell>` with ring-buffer indexing**

A flat `Vec<Cell>` of size `max_scrollback * cols` with a head pointer is more cache-friendly than `VecDeque<Vec<Cell>>` because all scrollback cells are contiguous. Cache benefit: scanning 5000 × 220 = 1,100,000 cells for a search-in-scrollback feature would be faster. Cost: the flat buffer must be allocated upfront at full size, even if the user has only produced 10 lines of output. At 220 cols × 16 bytes × 5000 rows = 17.6 MB, this is a significant upfront allocation. `VecDeque<Vec<Cell>>` grows lazily: at 100 lines of scrollback, only 100 × 220 × 16 = 352 KB is allocated. For most interactive sessions that never fill their scrollback, the lazy model is strictly better.

**Rope-like structure (e.g. `Rope` from the `ropey` crate)**

A rope stores text as a tree of chunks, enabling O(log n) insertion and deletion at arbitrary positions. This is optimal for text editors where the user inserts/deletes at arbitrary points. Terminal scrollback only ever appends at the tail and discards at the head — a deque is O(1) for both operations. A rope adds complexity and pointer chasing with no benefit over a deque for this access pattern.

**mmap'd file for large scrollbacks**

Writing scrollback to a temp file via `mmap` would allow scrollbacks measured in millions of lines without RAM pressure. Used by some multiplexers (tmux's `history-file`). Problems: (a) `mmap` on Windows is an entirely different API than POSIX `mmap`, requiring a Windows-specific code path; (b) writes to an mmap'd file are still bounded by page faults and dirty page writebacks — they are not "free" in the way RAM writes are for interactive scrollback; (c) implementing a correct mmap'd ring buffer with proper locking for the reader/writer separation is substantial complexity. For the default 5000-line limit, `VecDeque<Vec<Cell>>` costs at most 17.6 MB — within budget. Add mmap-backed scrollback only if user demand for 100k+ line scrollbacks materializes in Phase 4+.

**`VecDeque<Row>` where `Row = SmallVec<[Cell; 80]>`**

Using `SmallVec` for rows avoids heap allocation for rows shorter than 80 columns. Terminal rows are always full-width (the PTY has a fixed column count). `SmallVec`'s stack storage would never activate for 220-column rows. Not applicable.

### Memory budget

5000 lines × 220 cols × 16 bytes = 17.6 MB worst case. Plus `VecDeque` overhead (two heap allocations per row: one for the `VecDeque` element array, one for each row's `Vec<Cell>`). In practice, `VecDeque` allocates its element array in one contiguous block; individual row `Vec<Cell>`s are separate allocations. For 5000 rows, this is 5000 individual heap allocations of 3.5 KB each. Fragmentation is a theoretical concern but not a practical issue on modern allocators (jemalloc/mimalloc/system malloc).

### Trade-offs accepted

- `VecDeque<Vec<Cell>>` means row access is O(1) (deque index + row pointer follow) but each row is a separate allocation. A tight scrollback search would generate pointer chasing. Acceptable for Phase 1–3; add a flat buffer option in Phase 4+ if needed.
- Cloning a row for push to scrollback costs `cols` Cell copies (220 × 16 = 3.5 KB memcpy). At terminal scroll speeds (100 lines/second during `cat bigfile`), this is 350 KB/s of memcpy — negligible.

---

## 5. Cell Representation

### Decision

```rust
#[derive(Clone, Copy, PartialEq)]
pub struct Cell {
    pub ch: char,          // Unicode scalar value, 4 bytes
    pub fg: Color,         // 4 bytes (see below)
    pub bg: Color,         // 4 bytes
    pub flags: CellFlags,  // u8 bitset, 1 byte
    // 3 bytes padding → 12 bytes total (or 16 with Color as [u8;4])
}

#[derive(Clone, Copy, PartialEq)]
pub enum Color {
    Default,
    Indexed(u8),           // ANSI 256-color palette index
    Rgb(u8, u8, u8),       // True color (24-bit)
}
// Color stored as a tagged union: 1 discriminant byte + 3 data bytes = 4 bytes

bitflags::bitflags! {
    pub struct CellFlags: u8 {
        const BOLD       = 0b0000_0001;
        const ITALIC     = 0b0000_0010;
        const UNDERLINE  = 0b0000_0100;
        const BLINK      = 0b0000_1000;
        const INVERSE    = 0b0001_0000;
        const STRIKETHROUGH = 0b0010_0000;
        const INVISIBLE  = 0b0100_0000;
        const WIDE_CHAR  = 0b1000_0000;  // this cell is the left half of a wide (2-col) char
    }
}
```

Total cell size: 4 (ch) + 4 (fg) + 4 (bg) + 1 (flags) + 3 (padding) = 16 bytes.

### Why `char` over `u32`

Rust's `char` is a valid Unicode scalar value (0x0000–0xD7FF, 0xE000–0x10FFFF). It is stored as a `u32` at the machine level (4 bytes). Using `char` instead of `u32` directly gives two benefits: (a) the Rust type system prevents storing surrogate values, which are not valid Unicode scalar values; (b) `char::len_utf8()` and `char::encode_utf8()` are available without casting. The cost is zero.

### Wide character handling (CJK, emoji)

Wide characters (East Asian Width: W or F, i.e. fullwidth) occupy two terminal columns. Emoji can be wide via the Unicode Emoji data. The model used by virtually all terminal emulators (xterm, WezTerm, Alacritty):

1. The wide character's Unicode scalar is stored in the left cell with `CellFlags::WIDE_CHAR` set.
2. The right (second-column) cell stores `'\0'` (or a sentinel `char`) with `CellFlags::WIDE_PLACEHOLDER` (add to CellFlags if needed).
3. The renderer checks `WIDE_CHAR`: if set, render the glyph at double cell width, skip the placeholder cell.

```rust
// Writing a wide char at (col, row):
fn write_wide_char(&mut self, col: u16, row: u16, ch: char, fg: Color, bg: Color, flags: CellFlags) {
    debug_assert!(col + 1 < self.cols, "wide char at last column");
    let idx = row as usize * self.cols as usize + col as usize;
    self.cells[idx] = Cell { ch, fg, bg, flags: flags | CellFlags::WIDE_CHAR };
    self.cells[idx + 1] = Cell { ch: '\0', fg, bg, flags: CellFlags::empty() };
    self.dirty[idx] = true;
    self.dirty[idx + 1] = true;
}
```

Width detection uses the `unicode-width` crate:
```rust
use unicode_width::UnicodeWidthChar;
let width = ch.width().unwrap_or(1);
if width == 2 { write_wide_char(...) } else { write_char(...) }
```

### Grapheme cluster handling

Some Unicode sequences are multi-codepoint grapheme clusters: e.g. family emoji (👨‍👩‍👧 = U+1F468 ZWJ U+1F469 ZWJ U+1F467), combining characters (e + combining grave = è). The VT specification and most terminal emulators handle grapheme clusters by storing only the base character in the cell and either ignoring or combining subsequent combining characters.

For Phase 1–3, store only the first codepoint of each cluster in the cell. Combining characters (General Category Mn, Mc) received as `vte::Perform::print` should: check if the previous cell cursor position has a base character; if so, update that cell's char to the composed form (use `unicode_normalization::char::compose(base, combiner)`); do not advance the cursor. For unrecognized or complex multi-codepoint sequences, store the base codepoint and discard the rest.

Full grapheme cluster support (storing a `SmallVec<[char; 4]>` per cell) is a Phase 5+ enhancement. It adds 24+ bytes per cell (tripling cell size) and is needed only for complex scripts (Devanagari, Arabic, some emoji sequences). Most terminal users never encounter these.

### `CellFlags` bitset design

Using `bitflags!` macro gives:
- Type-safe set operations: `flags | CellFlags::BOLD`
- Free conversion to/from `u8` via `.bits()` / `CellFlags::from_bits_truncate()`
- 8 flag bits is sufficient for Phase 1–3. If a 9th flag is needed, expand to `u16` (1 byte wasted vs `u8`, but trivial to change).

SGR (Select Graphic Rendition) code mapping:
```
SGR 1  → BOLD
SGR 3  → ITALIC
SGR 4  → UNDERLINE
SGR 5  → BLINK
SGR 7  → INVERSE
SGR 9  → STRIKETHROUGH
SGR 8  → INVISIBLE
```
Reset (SGR 0 or SGR 22/23/24/25/27) clears the respective bit.

### Trade-offs accepted

- `char` as `u32` means cells that have no character (right half of wide chars, empty lines after resize) store `'\0'`. The renderer must handle `'\0'` explicitly (draw nothing / skip glyph upload for placeholder cells).
- `Color` as a tagged enum with `Default`, `Indexed(u8)`, `Rgb(u8,u8,u8)` is 4 bytes total. Storing it as two `u32` values (encoded color + tag) would be 8 bytes but simpler to pass to the GPU. The current design is fine; conversion to `[f32; 4]` happens at render time, not in the cell.
- No support for 256-color palette lookups inside `Cell` — palette resolution (`Indexed(n)` → `Rgb(r,g,b)`) happens in the renderer using the active color palette from config. This is correct: `Indexed` values can change meaning when the application remaps the palette (OSC 4).

---

## 6. Async Architecture — Tokio + Dedicated PTY Reader Thread

### Decision

Use `tokio` as the async runtime. The PTY reader runs on a **dedicated OS thread** (not a tokio task), pushing bytes into a `tokio::sync::mpsc` channel. The tokio runtime processes the channel on the main async task, calls `vte::Parser::advance`, and mutates the `Grid`. Keystroke input flows from the winit event loop through a second `mpsc::Sender<Vec<u8>>` to a tokio task that writes to the PTY's writer.

```
OS Thread: PTY Reader
  ┌─────────────────────────────────────┐
  │  loop { reader.read(&mut buf)       │
  │         tx_pty.blocking_send(buf)   │ ← tokio::sync::mpsc (bounded, 32 items)
  │  }                                  │
  └─────────────────────────────────────┘
           ↓
Tokio Task: VTE Parser + Grid Mutator
  ┌─────────────────────────────────────┐
  │  while let Some(bytes) = rx.recv()  │
  │    for b in bytes:                  │
  │      parser.advance(&mut perf, b)   │ ← mutates Grid
  │  notify renderer (dirty_tx.send())  │
  └─────────────────────────────────────┘
           ↓
Winit Event Loop (main thread)
  ┌─────────────────────────────────────┐
  │  on RedrawRequested:                │
  │    renderer.render(grids)           │
  └─────────────────────────────────────┘

Winit Event Loop → Tokio Task: PTY Writer
  ┌─────────────────────────────────────┐
  │  on KeyboardInput:                  │
  │    tx_input.send(bytes)             │
  └─────────────────────────────────────┘
           ↓
Tokio Task: PTY Writer
  ┌─────────────────────────────────────┐
  │  while let Some(b) = rx_input.recv()│
  │    pty_writer.write_all(b)          │
  └─────────────────────────────────────┘
```

### Why a dedicated OS thread for the PTY reader (not a tokio task)

`portable-pty`'s `try_clone_reader()` returns a blocking `std::io::Read` implementation. Blocking reads must never be performed inside a tokio async task on the tokio runtime's thread pool, because a blocking read stalls the worker thread and can starve other tasks. The correct approach is `tokio::task::spawn_blocking`, which moves the work to a separate blocking thread pool — but this creates a new thread per `await` call, which is inefficient for a tight read loop. A single dedicated OS thread with `blocking_send` (which parks the thread until the channel has capacity) is cleaner, more predictable, and avoids tokio's blocking thread pool overhead. One thread per PTY session.

### Why `tokio::sync::mpsc` over crossbeam-channel

`crossbeam-channel` is an excellent MPSC/MPMC channel with better throughput than `std::sync::mpsc` for multi-producer scenarios. However, Gunter uses `tokio` as its async runtime, and tokio's own `mpsc` channel is `async/await` native — `.recv().await` integrates with the tokio scheduler without blocking the OS thread. Mixing `crossbeam-channel`'s synchronous API with an async receiver requires either `spawn_blocking` (which defeats the purpose) or a custom waker, neither of which is worth implementing when `tokio::sync::mpsc` already does the right thing. Use `crossbeam-channel` only for sync-to-sync communication (never in this codebase).

### Why `tokio::sync::mpsc` over `flume`

`flume` is a faster, unified sync/async channel. It performs marginally better than `tokio::sync::mpsc` in microbenchmarks and supports both sync and async endpoints on the same channel. For Gunter's PTY throughput (peak ~1 MB/s of raw PTY bytes, chunked in 4096-byte reads), neither channel implementation is a bottleneck. Use `tokio::sync::mpsc` for consistency with the rest of the tokio ecosystem. No mixed-sync-async use case justifies `flume`.

### Channel sizing

The PTY→VTE channel is bounded at 32 items (each item is a `Vec<u8>` buffer of up to 4096 bytes). This provides:
- 32 × 4096 = 128 KB of in-flight PTY data before the reader thread blocks (backpressure)
- Backpressure is correct: if the VTE parser is overwhelmed (e.g. parsing 10 MB of `cat` output), the reader thread parks instead of allocating unbounded memory.

The input→PTY writer channel is bounded at 64 items. Keystrokes are small (1–8 bytes each) and infrequent. A bound of 64 prevents any theoretically unbounded allocation from rapid keyboard input.

### Why tokio over async-std or smol

`portable-pty`, `wezterm-font`, and the broader ecosystem of server crates all target `tokio`. Using a different async runtime would require bridging layers (`async-compat`) or finding equivalents for all tokio-native utilities (`tokio::sync`, `tokio::net`, `tokio::fs`). No compelling advantage justifies the ecosystem friction.

### Trade-offs accepted

- One OS thread per PTY session. For a typical window with 4 panes: 4 PTY reader threads + 1 tokio runtime thread + 1 winit main thread = 6 threads. This is acceptable.
- The PTY reader thread is a tight `loop { read; send }` which cannot be cancelled cleanly by dropping the channel sender from the tokio side without the OS thread noticing. Add a `shutdown: Arc<AtomicBool>` that the session manager sets on destroy, checked in the read loop.
- `tokio::sync::mpsc::Sender::blocking_send` requires calling `tokio::runtime::Handle::block_on` if used from outside a tokio context — the dedicated thread is not inside tokio, so `blocking_send` is the correct API (it does not require a tokio context).

---

## 7. Layout Tree — Binary Split Tree

### Decision

Use a recursive binary split tree as specified in `gunter-core/src/layout.rs`:

```rust
pub enum Layout {
    Leaf(Uuid),
    Split {
        axis:  Axis,
        ratio: f32,       // 0.0 < ratio < 1.0; left/top gets ratio * parent_size
        left:  Box<Layout>,
        right: Box<Layout>,
    },
}

pub enum Axis { Horizontal, Vertical }
```

Each tab maintains one root `Layout`. `Leaf(id)` maps to a session UUID. `Split` divides its rect between two children. The reflow function is a recursive tree walk:

```rust
pub fn reflow(layout: &Layout, rect: Rect, sessions: &mut HashMap<Uuid, Session>) {
    match layout {
        Layout::Leaf(id) => {
            let s = sessions.get_mut(id).unwrap();
            s.rect = rect;
            let (cols, rows) = rect.cell_dimensions(s.cell_size);
            s.grid.resize(cols, rows);
            s.pty.resize(PtySize { cols, rows, pixel_width: rect.w, pixel_height: rect.h }).ok();
        }
        Layout::Split { axis, ratio, left, right } => {
            let (lr, rr) = rect.split(*axis, *ratio);
            reflow(left,  lr, sessions);
            reflow(right, rr, sessions);
        }
    }
}
```

### Why binary split tree over flexbox-like layout

Flexbox (CSS Flexbox model) allows items to grow/shrink with weights, wrapping, and alignment. It is expressive but complex. For a terminal pane model, the user mental model is tmux/i3: split a pane in half, get two equal panes. The binary split tree captures this exactly. Flexbox's expressive power (multi-child flex containers, min/max sizes, wrap behavior) is not needed and would complicate the resize algorithm, the split/close operations, and the "find adjacent pane" navigation algorithm. Use the simpler model.

### Why binary split tree over a tiling constraint solver

Some tiling window managers (e.g. XMonad's layout algorithms) use constraint-based layout where the geometry is solved from a set of constraints. This is powerful for complex layout rules but overkill for a terminal multiplexer. `ratio` in the split node is the only constraint; the tree structure encodes all topology information. The constraint system would add an entire solving library dependency for no practical benefit.

### Why the same model as i3/tmux

- User mental model matches: tmux and i3 users already understand binary split trees. "Split horizontal" = one `Split { axis: Horizontal, ... }` node.
- Navigation algorithms are well-known: "go to pane left" traverses the tree to find the adjacent `Leaf` in the split direction.
- Operations (split, close, resize, zoom) map cleanly to tree mutations:
  - **Split**: replace `Leaf(id)` with `Split { left: Leaf(id), right: Leaf(new_id), ratio: 0.5 }`
  - **Close**: replace `Split` node with the surviving child
  - **Resize**: update `ratio` on the containing `Split` node, call `reflow`
  - **Zoom**: store the full-screen rect, call `reflow(leaf, full_rect)`

### Resize reflow algorithm

On window resize (winit `ResizeRequested`):
1. Compute the new root `Rect` from window pixel size minus any chrome (title bar, tab bar).
2. Call `reflow(root_layout, new_root_rect, sessions)`.
3. `reflow` recursively splits the rect by `ratio` at each `Split` node.
4. At each `Leaf`, `session.pty.resize(PtySize { cols, rows, pixel_width, pixel_height })` is called.
5. On POSIX, `portable-pty` sends `SIGWINCH` to the child process. On Windows, ConPTY handles resize internally.
6. `grid.resize(cols, rows)` reinitializes the cell buffer.
7. `grid.all_dirty = true`.

Note: `ratio` is stored as a float to allow non-50% splits. When the user drags a split divider, update `ratio` and call `reflow`. The tree structure does not change; only the ratio changes.

### Finding the adjacent pane (navigation)

Given the focused `Leaf(id)`, navigate to the pane "to the left":

```rust
fn find_adjacent(layout: &Layout, target: Uuid, direction: Direction) -> Option<Uuid> {
    // Walk the tree; at each Split, if the target is in the right/bottom subtree
    // and the split axis matches the direction, return the rightmost Leaf in the left/top subtree.
    // Full implementation: DFS with parent tracking.
}
```

This is the same algorithm i3 uses. It is O(n) where n is the number of panes — negligible.

### Trade-offs accepted

- `ratio: f32` can drift due to floating-point arithmetic over repeated resizes. Clamp to `[0.05, 0.95]` to prevent panes from becoming too small to display.
- `Box<Layout>` means the tree is heap-allocated. For a typical window with 4–8 panes, this is 4–8 allocations of 24–48 bytes each. Not a performance concern.
- No support for equal-tiling layouts (all panes equal width/height) without the user manually setting every ratio to `0.5`. This is acceptable; add a "balance panes" command in Phase 3+ if requested.

---

## 8. Session Lifecycle — Create / Destroy / Attach

### Decision

A `Session` represents one PTY + `Grid` + parser pair, identified by a `Uuid`. Sessions live in a `HashMap<Uuid, Session>` owned by the window state.

```rust
pub struct Session {
    pub id:         Uuid,
    pub pty:        Box<dyn MasterPty + Send>,
    pub pty_child:  Box<dyn Child + Send>,
    pub grid:       Grid,
    pub parser:     vte::Parser,
    pub tx:         mpsc::Sender<Vec<u8>>,    // keystrokes → PTY stdin
    pub rx_output:  mpsc::Receiver<Vec<u8>>,  // ← PTY reader thread
    pub rect:       Rect,
    pub title:      String,
    pub shutdown:   Arc<AtomicBool>,
}
```

### Session creation

```
1. Generate id = Uuid::new_v4()
2. portable_pty: openpty(PtySize) → PtyPair
3. CommandBuilder with shell, env vars (TERM, COLORTERM, etc.)
4. PtyPair.slave.spawn_command(cmd) → Box<dyn Child>
5. PtyPair.master.try_clone_reader() → blocking reader
6. PtyPair.master.take_writer() → blocking writer
7. Spawn PTY reader OS thread: loop { read; tx.blocking_send(buf) }
8. Spawn PTY writer tokio task: loop { rx.recv(); writer.write_all }
9. Spawn VTE processor tokio task: loop { rx_output.recv(); parser.advance; notify_render }
10. Insert Session into HashMap<Uuid, Session>
11. Insert Leaf(id) into layout tree at the correct position
```

### Session destruction

```
1. Set session.shutdown.store(true) → PTY reader thread exits on next read
2. Drop tx → PTY writer task sees channel closed, exits
3. Drop pty → PTY master fd closed → child process receives HUP
4. pty_child.wait() (non-blocking: try_wait, or in a background task)
5. Remove Leaf(id) from layout tree → replace with sibling (collapse Split node)
6. Remove Session from HashMap
7. Call reflow on the modified layout tree
```

### Zombie PTY cleanup

When the child process exits (shell `exit`, crash), the PTY reader thread's `read()` returns `0` or an error. The thread sends a `SessionEvent::ChildExited(id)` through a separate event channel to the main tokio loop. The main loop then triggers the same destruction sequence as above.

```rust
// In PTY reader thread:
let n = reader.read(&mut buf)?;
if n == 0 {
    let _ = event_tx.blocking_send(SessionEvent::ChildExited(session_id));
    break;
}
```

Do not leave zombie sessions in the `HashMap`. A zombie session consumes a PTY fd, a thread, and a `Grid` allocation indefinitely.

### Attach semantics (socket server)

When an external client attaches via the socket server (Section 9), it does not create a new session. It sends a session UUID and receives the current grid state plus a live byte stream from the PTY master. The session's `Grid` is serialized to the client at attach time (as a sequence of synthetic VT sequences that reconstruct the current screen, or as a raw cell dump). Subsequent PTY output is tee'd: both the grid-mutator task and the socket client receive the bytes. Detach is the client closing the socket connection.

### Session UUID mapping

`Uuid::new_v4()` generates a random 128-bit UUID. The UUID is used as:
- The `Leaf(Uuid)` identifier in the layout tree
- The key in the `HashMap<Uuid, Session>`
- The socket path suffix: `/tmp/gunter-{id}.sock` or `\\.\pipe\gunter-{id}`
- The `--attach <uuid>` argument for `gunter attach`

UUIDs are never reused within a process lifetime. After a session is destroyed, its UUID is gone.

### Trade-offs accepted

- `HashMap<Uuid, Session>` lookup is O(1) amortized with a 16-byte key (UUID). The hash is computed with a non-cryptographic hasher (`FxHashMap` from `rustc-hash` or `AHashMap` from `ahash`) since security is not a concern for session IDs within a local process.
- The PTY reader thread has no clean cancellation mechanism beyond `shutdown: Arc<AtomicBool>`. On the OS, the PTY master fd close will unblock the blocking `read()` with an error, so the thread exits promptly when the session is destroyed.
- `pty_child.wait()` should be called from a background tokio task to avoid blocking. If the child does not exit promptly after HUP (e.g. a process ignoring SIGHUP), do not `wait` indefinitely. After a 5-second timeout, log a warning and move on. The OS will reap the orphan eventually.

---

## 9. Socket Server Design — Raw Byte Stream

### Decision

Implement an optional socket server (`gunter-core/src/server.rs`) that exposes each session as a raw byte-stream socket:

- **Windows**: Named pipe `\\.\pipe\gunter-{session-uuid}`
- **Linux/macOS/WSL**: Unix domain socket `/tmp/gunter-{session-uuid}.sock`

Protocol: raw bytes in both directions. Bytes written to the socket by the client are forwarded to the PTY master's stdin. Bytes read from the PTY master's stdout are forwarded to the socket client. No framing, no length prefixes, no JSON.

```rust
// Server skeleton (Linux path shown)
use tokio::net::UnixListener;

pub async fn serve_session(session_id: Uuid, mut pty_rx: BroadcastReceiver<Bytes>, pty_tx: Sender<Vec<u8>>) {
    let path = format!("/tmp/gunter-{}.sock", session_id);
    let listener = UnixListener::bind(&path).unwrap();
    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let (mut reader, mut writer) = socket.into_split();
        // Forward PTY output to socket client:
        tokio::spawn(async move {
            while let Ok(bytes) = pty_rx.recv().await {
                if writer.write_all(&bytes).await.is_err() { break; }
            }
        });
        // Forward socket client input to PTY:
        tokio::spawn(async move {
            let mut buf = [0u8; 256];
            loop {
                let n = reader.read(&mut buf).await.unwrap_or(0);
                if n == 0 { break; }
                let _ = pty_tx.send(buf[..n].to_vec()).await;
            }
        });
    }
}
```

### Why raw bytes over a framed protocol

**Framed protocol (length-prefixed messages)**

Framing (e.g. 4-byte big-endian length + payload) allows sending structured messages: resize events, title changes, session metadata. But for the attach use case (external tools connecting to a running session), the client already speaks PTY: it sends VT sequences (ANSI escape codes) for terminal input and expects raw PTY output. Framing would require the client to implement a custom protocol instead of treating the socket as a transparent PTY. This breaks the "attach like tmux" model.

**JSON-RPC or msgpack protocol**

Higher-level protocols allow querying session metadata, listing sessions, creating/destroying sessions remotely. This is appropriate for a control socket (a separate admin channel). For the data channel (PTY I/O), JSON-RPC is wrong: JSON cannot efficiently transport binary PTY data, and the overhead of framing + encoding + decoding at 500 KB/s PTY throughput is not negligible. Separate concerns: if a control protocol is needed (Phase 4+), add a separate control socket with JSON-RPC. The data socket stays raw.

**tmux's approach as validation**

tmux exposes sessions via a control mode (`tmux -CC`) which is a framed text protocol on top of a raw PTY. The `attach-session` command does not use this: it simply redirects the client's terminal stdin/stdout to the PTY master, which is the raw byte model. `gunter attach` should work the same way: open the socket, set the client's terminal to raw mode, and forward bytes. Any standard tool (nc, socat, a custom client) can then attach with:
```
nc -U /tmp/gunter-{id}.sock
```

**Why not a single global socket for all sessions**

A single socket (e.g. `/tmp/gunter.sock`) multiplexing all sessions would require a session-selection protocol — clients would need to send a session ID before getting routed to the right PTY. This adds framing complexity. One socket per session is simpler and matches the 1:1 mapping between PTY and socket. The `gunter list` command can enumerate available socket paths.

### Windows named pipe considerations

On Windows, `\\.\pipe\gunter-{id}` is a named pipe. `tokio::net` does not natively support Windows named pipes on all versions; use `tokio::net::windows::named_pipe::ServerOptions` (available in tokio 1.15+). The client-side equivalent: `tokio::net::windows::named_pipe::ClientOptions::new().open(path)`. Raw byte semantics are identical to Unix sockets.

### Trade-offs accepted

- Raw byte stream means the socket server cannot send resize events to the client without an out-of-band mechanism. For Phase 4, the client sends resize as an ANSI sequence (CSI 8 ; rows ; cols t) or via a separate control socket. The PTY master processes this ANSI sequence and resizes accordingly.
- One socket per session: for 4 panes, there are 4 sockets. The attach client must know which session UUID to connect to. `gunter list` command prints available sessions with titles.
- The socket file is not cleaned up on unclean shutdown (process crash). Add a cleanup in the session destroy path, plus a startup scan that removes stale sockets (check if the socket has a living owner via the abstract namespace or a lock file).

---

## 10. Multi-Shell Support — `CommandBuilder` Abstraction

### Decision

Use `portable-pty`'s `CommandBuilder` for all shell spawning. `CommandBuilder` is shell-agnostic: pass the executable path and any arguments.

```rust
use portable_pty::CommandBuilder;

fn build_command(config: &ShellConfig) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(&config.program);
    for arg in &config.args {
        cmd.arg(arg);
    }
    // Mandatory environment variables for correct terminal behavior:
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // Propagate the user's existing environment:
    cmd.env_inherit();   // or manually copy relevant vars
    // Nerd Font awareness (some tools check this):
    cmd.env("TERM_PROGRAM", "gunter");
    cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
    cmd
}
```

### Shell detection

Shell is configured in `config.toml`:
```toml
[shell]
program = "wsl.exe"
args    = []
```

For a new-tab dialog that lets the user pick the shell, enumerate available shells at startup:
1. **WSL**: check if `wsl.exe` exists in PATH.
2. **PowerShell 7**: check for `pwsh.exe` in PATH.
3. **PowerShell 5**: check for `powershell.exe`.
4. **CMD**: check for `cmd.exe` (always present on Windows).
5. **POSIX shells**: check `SHELL` environment variable (on Linux/macOS), then fallback to `bash`, `zsh`, `sh`.

Do not hardcode shell paths. Use `which`-style PATH resolution: `std::env::var("PATH").split(":").find(|dir| dir.join(program).exists())`.

### Environment variable propagation

Correct terminal behavior requires these variables in the child's environment:

| Variable | Value | Why |
|---|---|---|
| `TERM` | `xterm-256color` | Tells apps (vim, tmux, etc.) what escape sequences the terminal supports. Never `vt100` (too limited) or `xterm` (no 256-color). |
| `COLORTERM` | `truecolor` | Signals that 24-bit color is supported. Without this, apps like neovim fall back to 256-color. |
| `TERM_PROGRAM` | `gunter` | App-level terminal identification. Used by some tools to enable terminal-specific features. |
| `TERM_PROGRAM_VERSION` | `0.1.0` | Pair with `TERM_PROGRAM`. |
| `COLUMNS` | `80` (initial) | Some apps read this at startup. The PTY resize (`SIGWINCH`) updates it automatically via `ioctl` on POSIX. |
| `LINES` | `24` (initial) | Same as `COLUMNS`. |

Do not override `HOME`, `USER`, `PATH`, or `SHELL` — inherit from the parent process. On Windows with WSL, these are set correctly inside the WSL environment regardless.

### Why `TERM=xterm-256color` and not `xterm-kitty`

kitty's terminal identifier (`TERM=xterm-kitty`) enables kitty's extended protocol (sixel, graphics protocol, keyboard encoding). Gunter does not implement the kitty graphics protocol in Phases 1–3. Setting `TERM=xterm-kitty` would cause apps that detect this (e.g. nvim with the kitty keyboard protocol) to send escape sequences Gunter does not handle. Use `xterm-256color` as the safe default. Add a `TERM=xterm-gunter` with a custom terminfo entry in Phase 5 when custom protocol extensions are implemented.

### Windows: CMD and PowerShell specifics

- **CMD**: no `TERM` support; `COLORTERM` is ignored. CMD uses Windows Console API internally. Through ConPTY, CMD works correctly — it detects that it is running inside a ConPTY and outputs VT sequences automatically (Windows 10 1903+).
- **PowerShell 5 (`powershell.exe`)**: uses Windows Console API; through ConPTY, emits VT sequences. Set `$env:TERM = 'xterm-256color'` via the shell config if PowerShell scripts check it.
- **PowerShell 7 (`pwsh.exe`)**: modern, cross-platform; respects `TERM` and `COLORTERM`.
- **WSL (`wsl.exe`)**: the wsl.exe process acts as a bridge; the actual shell inside WSL is a POSIX process and respects all POSIX terminal conventions.

### Trade-offs accepted

- Shell detection at startup adds ~5 ms to startup time. Cache the result.
- `cmd.env_inherit()` copies the parent process's entire environment into the child. On Windows, this environment can be large (hundreds of variables). It is still the correct behavior — do not manually whitelist variables as that would break PATH, user auth tokens, proxy settings, etc.

---

## 11. Input Routing — Focus Model and Broadcast Mode

### Decision

**Focus model**: exactly one pane (`Leaf`) is focused at any time per tab. The focused pane's UUID is stored in the tab state. Keyboard input goes exclusively to the focused pane.

```rust
pub struct Tab {
    pub layout:  Layout,
    pub focused: Uuid,   // must always be a valid Leaf UUID in this tab's layout
}
```

**Routing path**: `winit KeyboardInput` → `gunter-input` maps to `Action` → if `Action::SendBytes(bytes)`, send to `sessions[tab.focused].tx`.

**Broadcast mode**: when enabled (like tmux `synchronize-panes`), the same keystroke bytes are sent to all `Leaf` UUIDs in the current tab's layout tree:

```rust
fn focused_sessions(layout: &Layout) -> Vec<Uuid> {
    // Walk tree, collect all Leaf UUIDs
}

fn route_input(bytes: Vec<u8>, tab: &Tab, sessions: &HashMap<Uuid, Session>, broadcast: bool) {
    if broadcast {
        for id in focused_sessions(&tab.layout) {
            if let Some(s) = sessions.get(&id) {
                let _ = s.tx.try_send(bytes.clone());
            }
        }
    } else {
        if let Some(s) = sessions.get(&tab.focused) {
            let _ = s.tx.try_send(bytes.clone());
        }
    }
}
```

### Why the focused-pane model over event-routing alternatives

**Hover focus (focus follows mouse, no click needed)**

Used by some window managers. For a terminal multiplexer, hover focus is disorienting when the mouse passes over a status line or pane border — the user's keystrokes suddenly go to an unintended pane. Click-to-focus (or explicit `next_pane` keybind) is the tmux/i3 model and matches user expectations for a terminal environment. Use click-to-focus as default; add `focus_follows_mouse` as a config option.

**Event bus (keystrokes broadcast to all panes, each pane decides whether to accept)**

Overly complex. Each pane would need a predicate to decide whether to accept input. For the terminal use case, panes are dumb PTYs — they do not make routing decisions. The application layer (this code) makes all routing decisions.

### Focus invariant maintenance

When a pane is closed:
1. If `tab.focused == closed_id`, set `focused` to the sibling pane's UUID.
2. If there are no remaining panes in the tab, close the tab.

When a pane is created (split):
1. The new pane becomes focused.

When switching tabs: each tab remembers its own `focused` pane; restore it on tab switch.

### Mouse event routing

Mouse events (click, scroll, selection) must be routed to the pane whose `Rect` contains the mouse position, regardless of keyboard focus. The pane under the mouse cursor receives mouse events; the keyboard-focused pane receives key events. These can differ.

Mouse position → pane lookup:
```rust
fn pane_at(layout: &Layout, sessions: &HashMap<Uuid, Session>, x: f32, y: f32) -> Option<Uuid> {
    match layout {
        Layout::Leaf(id) => {
            let rect = sessions[id].rect;
            if rect.contains(x, y) { Some(*id) } else { None }
        }
        Layout::Split { left, right, .. } => {
            pane_at(left, sessions, x, y).or_else(|| pane_at(right, sessions, x, y))
        }
    }
}
```

### Broadcast mode (sync-panes equivalent)

Broadcast mode is toggled by a keybind (e.g. `Ctrl+Shift+B`). When active, a visual indicator (border color change or status bar icon) shows which tab has broadcast enabled. Broadcast sends the same raw bytes to all panes simultaneously. Each pane's PTY receives the bytes independently and processes them on its own shell. This matches tmux's `synchronize-panes` behavior exactly.

### Trade-offs accepted

- Click-to-focus requires a click event to transfer focus, adding one extra mouse click when navigating between panes. Acceptable; matches tmux/screen behavior.
- Broadcast mode clones `bytes` for each target pane. For 4 panes, this is 4 × N bytes per keypress (N ≤ 8 bytes for most keystrokes). Negligible.
- `tx.try_send` (non-blocking send) drops keystrokes if the input channel is full. This should never happen in practice (the channel has capacity 64, and the PTY writer drains it in microseconds). If it does happen (extremely slow PTY write), `try_send` failing is safer than blocking the winit event loop.

---

## 12. Terminal State Machine — `vte::Perform` Implementation

### Decision

Implement `vte::Perform` on `GridPerformer`. The implementation maps VT escape sequences to `Grid` mutations. Priority ordering for what to implement first:

1. **`print(c: char)`** — character output (most frequent operation)
2. **`execute(byte)`** — C0 control characters (LF, CR, BS, BEL, TAB, SI, SO)
3. **`csi_dispatch` with 'm' (SGR)** — colors and attributes
4. **`csi_dispatch` with 'A'/'B'/'C'/'D'/'H'/'f'** — cursor movement
5. **`csi_dispatch` with 'J'/'K'** — erase in display/line
6. **`csi_dispatch` with 'r' (DECSTBM)** — scroll region
7. **`csi_dispatch` with 'S'/'T'** — scroll up/down
8. **`csi_dispatch` with 'h'/'l'** — mode set/reset (alternate screen, mouse modes)
9. **`osc_dispatch`** — title (OSC 0, 2), color palette (OSC 4, 10, 11)
10. **`esc_dispatch`** — ESC 7/8 (DECSC/DECRC), ESC c (RIS), character set switching

Items 1–8 cover ~99% of what interactive shells and common terminal applications (vim, tmux, htop, git, fzf) need.

### `print(c: char)` — character output

```rust
fn print(&mut self, c: char) {
    use unicode_width::UnicodeWidthChar;
    let width = c.width().unwrap_or(1);
    let (col, row) = self.grid.cursor;
    
    if col >= self.grid.cols {
        // Auto-wrap: move to next line
        self.grid.cursor.0 = 0;
        self.grid.cursor.1 += 1;
        if self.grid.cursor.1 >= self.grid.scroll_bottom {
            self.grid.scroll_up(1);
            self.grid.cursor.1 = self.grid.scroll_bottom - 1;
        }
    }
    
    if width == 2 {
        self.grid.write_wide_char(col, row, c, self.fg, self.bg, self.flags);
        self.grid.cursor.0 += 2;
    } else {
        self.grid.write_char(col, row, c, self.fg, self.bg, self.flags);
        self.grid.cursor.0 += 1;
    }
}
```

### `execute(byte)` — C0 controls

```
0x08 BS  → cursor col - 1 (clamp to 0)
0x09 TAB → advance cursor to next tab stop (every 8 columns)
0x0A LF  → cursor row + 1; if at scroll_bottom, scroll_up(1)
0x0B VT  → same as LF
0x0C FF  → same as LF
0x0D CR  → cursor col = 0
0x07 BEL → emit a beep event (or flash; implement later)
0x0E SO  → switch to G1 character set (track current charset; implement later)
0x0F SI  → switch to G0 character set
```

LF behavior: if the cursor is at the bottom of the scroll region (`scroll_bottom`), call `grid.scroll_up(1)` to push the top line of the scroll region into scrollback and shift rows up. If the cursor is not at the scroll bottom, simply increment the row.

### `csi_dispatch` — CSI sequences

CSI sequences have the form `CSI P ... P I ... I F` where P are parameter bytes, I are intermediate bytes, F is the final byte (action character). The `vte` crate provides parsed parameters as a `Params` iterator.

Key CSI implementations:

```
CSI n A     → cursor up n rows (CUU)
CSI n B     → cursor down n rows (CUD)
CSI n C     → cursor forward n cols (CUF)
CSI n D     → cursor backward n cols (CUB)
CSI row;col H  → cursor position (CUP), 1-indexed
CSI n J     → erase in display: 0=below, 1=above, 2=all, 3=scrollback+all
CSI n K     → erase in line: 0=to end, 1=to start, 2=entire line
CSI n S     → scroll up n lines
CSI n T     → scroll down n lines
CSI top;bot r  → set scroll region (DECSTBM), 1-indexed
CSI ? 1049 h   → switch to alternate screen buffer
CSI ? 1049 l   → switch back to primary screen buffer
CSI ? 1000 h   → enable mouse reporting (X10 mode)
CSI ? 2026 h   → synchronized output (begin batch; hold rendering until CSI ? 2026 l)
```

**SGR (CSI m)** deserves special attention because it is the most complex:

```rust
fn apply_sgr(&mut self, params: &Params) {
    let mut iter = params.iter().peekable();
    while let Some(param) = iter.next() {
        match param[0] {
            0  => { self.fg = Color::Default; self.bg = Color::Default; self.flags = CellFlags::empty(); }
            1  => self.flags.insert(CellFlags::BOLD),
            3  => self.flags.insert(CellFlags::ITALIC),
            4  => self.flags.insert(CellFlags::UNDERLINE),
            5  => self.flags.insert(CellFlags::BLINK),
            7  => self.flags.insert(CellFlags::INVERSE),
            8  => self.flags.insert(CellFlags::INVISIBLE),
            9  => self.flags.insert(CellFlags::STRIKETHROUGH),
            22 => self.flags.remove(CellFlags::BOLD),
            // ... (other resets)
            30..=37 => self.fg = Color::Indexed(param[0] - 30),
            38 => self.fg = parse_extended_color(&mut iter),  // 256-color or truecolor
            39 => self.fg = Color::Default,
            40..=47 => self.bg = Color::Indexed(param[0] - 40),
            48 => self.bg = parse_extended_color(&mut iter),
            49 => self.bg = Color::Default,
            90..=97  => self.fg = Color::Indexed(param[0] - 90 + 8),  // bright fg
            100..=107 => self.bg = Color::Indexed(param[0] - 100 + 8), // bright bg
            _ => {}  // ignore unknown SGR codes
        }
    }
}

fn parse_extended_color(iter: &mut impl Iterator<Item = SubParams>) -> Color {
    // SGR 38;5;n → Indexed(n) (256-color)
    // SGR 38;2;r;g;b → Rgb(r,g,b) (truecolor)
    // Also handle colon-separated form: 38:5:n and 38:2:r:g:b
    match iter.next().map(|p| p[0]) {
        Some(5) => Color::Indexed(iter.next().map_or(0, |p| p[0] as u8)),
        Some(2) => {
            let r = iter.next().map_or(0, |p| p[0] as u8);
            let g = iter.next().map_or(0, |p| p[0] as u8);
            let b = iter.next().map_or(0, |p| p[0] as u8);
            Color::Rgb(r, g, b)
        }
        _ => Color::Default,
    }
}
```

### `osc_dispatch` — OSC sequences

OSC sequences have no action character; they are terminated by `BEL` or `ST`.

```
OSC 0 ; title BEL/ST  → set window + icon title
OSC 2 ; title BEL/ST  → set window title
OSC 4 ; n ; spec BEL/ST → change color palette entry n
OSC 10 ; spec BEL/ST  → set default foreground color
OSC 11 ; spec BEL/ST  → set default background color
OSC 52 ; c ; data BEL/ST → clipboard write (Phase 4+)
```

The title dispatch should emit through a channel to the UI layer (tab title bar update), not mutate the grid:
```rust
fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
    match params.first().and_then(|p| std::str::from_utf8(p).ok()) {
        Some("0") | Some("2") => {
            if let Some(title) = params.get(1).and_then(|p| std::str::from_utf8(p).ok()) {
                let _ = self.title_tx.as_ref().map(|tx| tx.try_send(title.to_string()));
            }
        }
        _ => {}
    }
}
```

### DCS handling (sixel, DECRQSS)

DCS sequences span `hook` (entry), multiple `put` calls (data bytes), and `unhook` (exit). Sixel graphics are a DCS payload. For Phase 1–3, implement DCS as a no-op (accept and discard data). Add sixel in Phase 4+ as an optional feature.

### Alternate screen buffer

Most full-screen applications (vim, tmux, less) switch to the alternate screen on entry and back to the primary screen on exit. Implementation:

```rust
pub struct Grid {
    // Primary screen:
    pub cells: Vec<Cell>,
    pub scrollback: VecDeque<Vec<Cell>>,
    // Alternate screen (no scrollback):
    pub alt_cells: Option<Vec<Cell>>,
    pub alt_cursor: Option<(u16, u16)>,
    pub using_alt_screen: bool,
}

impl Grid {
    pub fn enter_alt_screen(&mut self) {
        // Save primary cursor
        self.alt_cursor = Some(self.cursor);
        // Switch to alt cells (initialized to blank)
        self.alt_cells = Some(vec![Cell::default(); self.cols as usize * self.rows as usize]);
        self.using_alt_screen = true;
        self.cursor = (0, 0);
        self.all_dirty = true;
    }
    pub fn exit_alt_screen(&mut self) {
        self.alt_cells = None;
        self.cursor = self.alt_cursor.unwrap_or((0, 0));
        self.using_alt_screen = false;
        self.all_dirty = true;
    }
    pub fn active_cells(&self) -> &[Cell] {
        if self.using_alt_screen { self.alt_cells.as_deref().unwrap() } else { &self.cells }
    }
    pub fn active_cells_mut(&mut self) -> &mut [Cell] {
        if self.using_alt_screen { self.alt_cells.as_deref_mut().unwrap() } else { &mut self.cells }
    }
}
```

The renderer always reads `grid.active_cells()`. No renderer changes needed.

### Trade-offs accepted

- Unknown CSI/OSC codes are silently ignored. This is correct terminal behavior: ignore unrecognized sequences rather than aborting or showing garbage. Log them at `trace!` level in debug builds.
- The SGR colon-separated subparam form (`38:2:r:g:b`) is required by modern terminals (ITU T.416 standard). `vte`'s `Params` type supports sub-parameters; implement it from the start to avoid "wrong color in neovim" bug reports later.
- `synchronized output` (CSI ? 2026) means the application signals "start of frame" and "end of frame". The terminal should buffer output during the frame and render once at "end of frame". Implement in Phase 3: set a flag at CSI ? 2026 h, buffer dirty updates, flush at CSI ? 2026 l. Without this, fast output (e.g. terminal animations) can tear visually.

---

## Appendix: Decision Matrix

| Topic | Decision | Key reason to reject alternatives |
|---|---|---|
| PTY backend | `portable-pty` | Cross-platform ConPTY+POSIX, standalone crate, WezTerm-tested |
| VT parser | `vte` | Zero-alloc, correct state machine, standalone crate |
| Grid data structure | Flat `Vec<Cell>` | Cache locality, O(1) cell access, `memmove` scroll |
| Scrollback buffer | `VecDeque<Vec<Cell>>` | Lazy allocation vs 17.6 MB flat upfront; O(1) push/pop |
| Cell char type | `char` (Unicode scalar) | Type safety over raw `u32`; width APIs available |
| Wide chars | Left cell with `WIDE_CHAR` flag | Standard model used by all major terminal emulators |
| `CellFlags` | `bitflags!` on `u8` | 8 flags sufficient; type-safe set operations |
| Async runtime | `tokio` | Ecosystem consistency; `portable-pty` ecosystem aligns |
| PTY reader | Dedicated OS thread | Blocking `Read` cannot run in tokio task |
| Channel | `tokio::sync::mpsc` | Async-native; no async-sync bridging needed |
| Layout | Binary split tree | Matches tmux/i3 user model; simple O(n) operations |
| Resize reflow | Recursive tree walk | Matches tree structure; O(panes) |
| Session lifecycle | Uuid → `HashMap` | O(1) lookup; UUID as socket suffix |
| Zombie cleanup | `ChildExited` event channel | No blocking `wait`; no leaked PTY fds |
| Socket protocol | Raw byte stream | Client speaks PTY natively; no protocol overhead |
| Socket path | Per-session UUID suffix | One socket per session; simple `gunter attach <id>` |
| Multi-shell | `CommandBuilder` + config | Shell-agnostic; no hardcoded paths |
| `TERM` value | `xterm-256color` | Safe default; no unimplemented kitty extensions |
| Input routing | Focused-pane model | Matches tmux/i3; click-to-focus |
| Broadcast mode | Clone bytes to all panes | Matches tmux `synchronize-panes`; minimal complexity |
| VTE priority | print → C0 → SGR → cursor → erase → scroll → modes | Covers 99% of interactive shell use in order of frequency |
| Alt screen | Two cell buffers, `active_cells()` | Standard model; renderer changes not needed |
| Synchronized output | CSI ? 2026 (Phase 3) | Prevents visual tearing in terminal animations |
