// grid.rs — Grid, Cell, CellFlags, Color

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn black() -> Self { Color { r: 0, g: 0, b: 0 } }
    pub const fn white() -> Self { Color { r: 255, g: 255, b: 255 } }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct CellFlags: u8 {
        const NONE      = 0;
        const BOLD      = 0b00000001;
        const ITALIC    = 0b00000010;
        const UNDERLINE = 0b00000100;
        const BLINK     = 0b00001000;
        const INVERSE   = 0b00010000;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl Cell {
    /// Blank cell with Atom One Dark palette defaults.
    pub fn blank() -> Self {
        Cell {
            ch: ' ',
            fg: Color { r: 171, g: 178, b: 191 },  // #abb2bf
            bg: Color { r: 40,  g: 44,  b: 52  },  // #282c34
            flags: CellFlags::NONE,
        }
    }
}

pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub cursor: (u16, u16),
    pub dirty: Vec<bool>,
}

impl Grid {
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

    #[inline]
    fn idx(&self, x: u16, y: u16) -> usize {
        (y as usize) * (self.cols as usize) + (x as usize)
    }

    /// Write cell at (x,y), mark dirty. Silently ignores out-of-bounds.
    pub fn write_cell(&mut self, x: u16, y: u16, cell: Cell) {
        if x < self.cols && y < self.rows {
            let i = self.idx(x, y);
            self.cells[i] = cell;
            self.dirty[i] = true;
        }
    }

    /// Clear all dirty flags (call after each render pass).
    pub fn clear_dirty(&mut self) {
        for d in &mut self.dirty {
            *d = false;
        }
    }

    /// Move cursor, clamping to grid bounds.
    pub fn cursor_move(&mut self, x: u16, y: u16) {
        self.cursor = (
            x.min(self.cols.saturating_sub(1)),
            y.min(self.rows.saturating_sub(1)),
        );
    }
}

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
        let cell = Cell { ch: 'A', fg: Color::white(), bg: Color::black(), flags: CellFlags::NONE };
        g.write_cell(0, 0, cell);
        assert!(g.dirty[0]);
    }

    #[test]
    fn clear_dirty_resets_all_flags() {
        let mut g = Grid::new(80, 24);
        let cell = Cell { ch: 'B', fg: Color::black(), bg: Color::black(), flags: CellFlags::NONE };
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
