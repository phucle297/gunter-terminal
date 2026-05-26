# Gunter Terminal Correctness — Design Spec
**Date:** 2026-05-26  
**Status:** Approved

## Problem Summary

Four user-visible bugs after phase 7 baseline:

1. **Color overlay** — `CellFlags::INVERSE` stored but never applied in renderer. Reverse-video (fzf selection, vim visual mode) shows wrong colors.
2. **Font rendering wrong** — `cell_h = cell_w * 2` is a guess. Glyph placement ignores baseline (`ymin`) and left bearing (`xmin`). Text appears misaligned vertically.
3. **Fish DA1 warning** — Terminal never responds to `\x1b[c` (Primary Device Attributes). Fish times out, logs warning. Also missing: CPR `\x1b[6n`, ESC M, save/restore cursor.
4. **fzf / TUI UI breakage** — Three compounding bugs: missing `wrap_next` deferred-wrap flag (cursor drift), box-drawing characters absent from atlas (U+2500–U+257F rendered as spaces), INVERSE not applied.

---

## Architecture Notes

### PTY Write-Back Channel

DA1 and CPR responses require `GridPerformer` to write bytes back to the PTY master. Currently performer is write-only to `Grid`. Fix: pass `mpsc::SyncSender<Vec<u8>>` into `GridPerformer::new()` as `response_tx: Option<&mpsc::SyncSender<Vec<u8>>>`. Caller already holds `pty_tx`; pass a clone.

### Color Representation

Current `Color { r, g, b }` cannot distinguish "default foreground" from an explicit RGB that happens to match the default. INVERSE on a default-colored cell must invert to the terminal default background (not the stored RGB). Fix: introduce `TermColor` enum:
```rust
enum TermColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}
```
Resolve to actual RGB at render time using the active palette.

### Glyph Atlas

Static atlas (16×6 grid for ASCII 0x20–0x7E) must be extended to cover:
- Box drawing U+2500–U+257F (128 chars)
- Block elements U+2580–U+259F (32 chars)

For everything else: dynamic LRU glyph cache — on atlas miss, rasterize glyph, append to texture, evict least-recently-used slot when atlas is full.

### Deferred Wrap (`wrap_next`)

VT100 spec: when a character is printed in column N-1 (last col), the cursor does not immediately wrap. It stays in col N-1 with `wrap_next = true`. The wrap executes only when the *next* printable character arrives. This is critical for all screen-relative TUI apps (fzf, vim, htop).

Add `wrap_next: bool` to `Grid`. In `GridPerformer::print()`: if `wrap_next`, execute wrap first, then print. Set `wrap_next = true` when cursor reaches last col and `auto_wrap` is enabled.

---

## Phase Plan

### Phase 1 — Terminal Protocol (Critical)
Fixes fish warning, fzf cursor, all screen-relative TUI apps.

| ID | Task |
|----|------|
| P1.1 | `wrap_next` deferred-wrap in Grid + performer |
| P1.2 | PTY write-back channel in GridPerformer |
| P1.3 | DA1 response `\x1b[?62;c` on `\x1b[c` / `\x1b[0c` |
| P1.4 | CPR `\x1b[6n` → `\x1b[row;colR` |
| P1.5 | ESC M reverse index (scroll down at top of region) |
| P1.6 | ESC 7/8 save/restore cursor |
| P1.7 | `?1h/l` application cursor keys + translate_key update |
| P1.8 | `?7h/l` DECAWM flag |
| P1.9 | OSC dispatch stub (window title via `\x1b]0;...\x07`) |

### Phase 2 — Rendering Correctness (High)
Visual correctness for color and text attributes.

| ID | Task |
|----|------|
| P2.1 | Apply INVERSE flag in renderer (swap fg/bg per cell) |
| P2.2 | Introduce `TermColor` enum; update Grid, performer, renderer |
| P2.3 | Fix cell height from font metrics (ascender + \|descender\|) |
| P2.4 | Baseline-align glyphs (`y_off = ascender - m.ymax`) |
| P2.5 | Apply left bearing (`x_off = m.xmin.max(0)`) |
| P2.6 | Underline rendering in shader (1px line at cell bottom) |

### Phase 3 — Unicode / Glyph Coverage (High)
Without this, fzf/ncurses UIs are broken.

| ID | Task |
|----|------|
| P3.1 | Extend static atlas: box drawing U+2500–U+257F |
| P3.2 | Extend static atlas: block elements U+2580–U+259F |
| P3.3 | Dynamic LRU glyph cache for non-static chars |
| P3.4 | Wide char (CJK) double-width cell support |

### Phase 4 — Input Correctness (Medium)
| ID | Task |
|----|------|
| P4.1 | Shift+F-key sequences in translate_key |
| P4.2 | Focus events `?1004h/l` |
| P4.3 | Mouse all-motion `?1003h/l` |

### Phase 5 — Font Architecture (Month 2)
| ID | Task |
|----|------|
| P5.1 | Replace fontdue atlas with rustybuzz + swash or cosmic-text |
| P5.2 | Font fallback chain via fontconfig / font-kit |
| P5.3 | Bold/italic face variants (not flag-based rendering) |
| P5.4 | Config-driven font selection in gunter.toml |

---

## Test Strategy

Each phase: failing test first → impl → `cargo test` green → commit.

Key test cases:
- `wrap_next`: print 80 chars on 80-col grid → cursor at col 79, `wrap_next=true`; print one more → cursor at (1, 1)
- DA1: feed `\x1b[c` → response_tx receives `\x1b[?62;c`
- CPR: feed `\x1b[6n` at cursor (5, 3) → response_tx receives `\x1b[4;6R`
- INVERSE: cell with INVERSE flag → renderer swaps fg/bg before CellInstance
- Font metrics: cell_h == ascender + |descender| from font; glyph at baseline row
- Box drawing: `─` (U+2500) has non-zero UV in atlas
