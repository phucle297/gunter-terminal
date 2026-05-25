// grid.rs — Grid, Cell, CellFlags, Color

use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn black() -> Self {
        Color { r: 0, g: 0, b: 0 }
    }

    pub const fn white() -> Self {
        Color { r: 255, g: 255, b: 255 }
    }
}

// ---------------------------------------------------------------------------
// CellFlags
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    #[derive(Clone, Debug, PartialEq)]
    pub struct CellFlags: u8 {
        const BOLD      = 0b00000001;
        const ITALIC    = 0b00000010;
        const UNDERLINE = 0b00000100;
        const BLINK     = 0b00001000;
        const INVERSE   = 0b00010000;
    }
}

// ---------------------------------------------------------------------------
// Cell
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl Cell {
    /// A blank (space) cell with default terminal colours and no attributes.
    pub fn blank() -> Self {
        Cell {
            ch: ' ',
            fg: Color::white(),
            bg: Color::black(),
            flags: CellFlags::empty(),
        }
    }
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub cursor: (u16, u16),
    pub dirty: Vec<bool>,
}

impl Grid {
    /// Create a new grid of `cols` x `rows` blank cells.
    pub fn new(cols: u16, rows: u16) -> Self {
        let count = (cols as usize) * (rows as usize);
        Grid {
            cols,
            rows,
            cells: vec![Cell::blank(); count],
            scrollback: VecDeque::new(),
            cursor: (0, 0),
            dirty: vec![false; count],
        }
    }

    /// Write a cell at position (x, y) and mark that position dirty.
    ///
    /// The flat index is `y * cols + x`.
    pub fn write_cell(&mut self, x: u16, y: u16, cell: Cell) {
        let idx = (y as usize) * (self.cols as usize) + (x as usize);
        if idx < self.cells.len() {
            self.cells[idx] = cell;
            self.dirty[idx] = true;
        }
    }

    /// Clear all dirty flags (called after a render pass).
    pub fn clear_dirty(&mut self) {
        for d in &mut self.dirty {
            *d = false;
        }
    }

    /// Move the cursor to (x, y).
    pub fn cursor_move(&mut self, x: u16, y: u16) {
        self.cursor = (x, y);
    }
}

// ---------------------------------------------------------------------------
// Tests — written before implementation (TDD red→green)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_creation_has_correct_dimensions() {
        let g = Grid::new(80, 24);
        assert_eq!(g.cols, 80);
        assert_eq!(g.rows, 24);
        assert_eq!(g.cells.len(), 80 * 24);
    }

    #[test]
    fn cell_write_sets_dirty_flag() {
        let mut g = Grid::new(80, 24);
        let cell = Cell {
            ch: 'A',
            fg: Color { r: 255, g: 255, b: 255 },
            bg: Color { r: 0, g: 0, b: 0 },
            flags: CellFlags::empty(),
        };
        g.write_cell(0, 0, cell);
        assert!(g.dirty[0]);
    }

    #[test]
    fn clear_dirty_resets_all_flags() {
        let mut g = Grid::new(80, 24);
        let cell = Cell {
            ch: 'B',
            fg: Color { r: 0, g: 0, b: 0 },
            bg: Color { r: 0, g: 0, b: 0 },
            flags: CellFlags::empty(),
        };
        g.write_cell(5, 3, cell);
        g.clear_dirty();
        assert!(g.dirty.iter().all(|&d| !d));
    }

    #[test]
    fn cursor_move_updates_position() {
        let mut g = Grid::new(80, 24);
        g.cursor_move(10, 5);
        assert_eq!(g.cursor, (10, 5));
    }
}
