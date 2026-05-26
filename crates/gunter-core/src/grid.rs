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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl Cell {
    pub fn blank() -> Self {
        Cell {
            ch: ' ',
            fg: Color { r: 171, g: 178, b: 191 },
            bg: Color { r: 40,  g: 44,  b: 52  },
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
    pub scroll_top: u16,
    pub scroll_bot: u16,
    pub cursor_visible: bool,
    pub alt_active: bool,
    pub alt_cells: Vec<Cell>,
    pub alt_cursor: (u16, u16),
    pub bracketed_paste: bool,
    pub mouse_reporting: bool,
    pub mouse_sgr: bool,
    pub cursor_style: CursorStyle,
    pub scroll_offset: usize,
    pub wrap_next: bool,
    pub title: String,
}

const MAX_SCROLLBACK: usize = 5000;

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
            scroll_top: 0,
            scroll_bot: rows.saturating_sub(1),
            cursor_visible: true,
            alt_active: false,
            alt_cells: vec![Cell::blank(); count],
            alt_cursor: (0, 0),
            bracketed_paste: false,
            mouse_reporting: false,
            mouse_sgr: false,
            cursor_style: CursorStyle::Block,
            scroll_offset: 0,
            wrap_next: false,
            title: String::new(),
        }
    }

    #[inline]
    fn idx(&self, x: u16, y: u16) -> usize {
        (y as usize) * (self.cols as usize) + (x as usize)
    }

    pub fn write_cell(&mut self, x: u16, y: u16, cell: Cell) {
        if x < self.cols && y < self.rows {
            let i = self.idx(x, y);
            self.cells[i] = cell;
            self.dirty[i] = true;
        }
    }

    pub fn clear_dirty(&mut self) {
        for d in &mut self.dirty {
            *d = false;
        }
    }

    pub fn cursor_move(&mut self, x: u16, y: u16) {
        self.cursor = (
            x.min(self.cols.saturating_sub(1)),
            y.min(self.rows.saturating_sub(1)),
        );
        self.wrap_next = false;
    }

    pub fn mark_all_dirty(&mut self) {
        for d in &mut self.dirty {
            *d = true;
        }
    }

    /// Scroll up N lines within the scroll region. Rows pushed out go to scrollback.
    pub fn scroll_up(&mut self, n: u16) {
        let n = n as usize;
        let top = self.scroll_top as usize;
        let bot = self.scroll_bot as usize;
        let cols = self.cols as usize;
        if top > bot { return; }

        for _ in 0..n {
            if top == 0 && bot == self.rows as usize - 1 {
                let row: Vec<Cell> = self.cells[..cols].to_vec();
                self.scrollback.push_back(row);
                if self.scrollback.len() > MAX_SCROLLBACK {
                    self.scrollback.pop_front();
                }
            }
            for row in top..bot {
                let src = (row + 1) * cols;
                let dst = row * cols;
                self.cells.copy_within(src..src + cols, dst);
            }
            for col in 0..cols {
                let i = bot * cols + col;
                self.cells[i] = Cell::blank();
            }
        }
        self.mark_all_dirty();
    }

    /// Scroll down N lines within the scroll region.
    pub fn scroll_down(&mut self, n: u16) {
        let n = n as usize;
        let top = self.scroll_top as usize;
        let bot = self.scroll_bot as usize;
        let cols = self.cols as usize;
        if top > bot { return; }

        for _ in 0..n {
            for row in (top..bot).rev() {
                let src = row * cols;
                let dst = (row + 1) * cols;
                self.cells.copy_within(src..src + cols, dst);
            }
            for col in 0..cols {
                let i = top * cols + col;
                self.cells[i] = Cell::blank();
            }
        }
        self.mark_all_dirty();
    }

    /// Enter alternate screen: swap cells/cursor.
    pub fn enter_alt(&mut self) {
        if self.alt_active { return; }
        std::mem::swap(&mut self.cells, &mut self.alt_cells);
        std::mem::swap(&mut self.cursor, &mut self.alt_cursor);
        for c in &mut self.cells { *c = Cell::blank(); }
        self.alt_active = true;
        self.mark_all_dirty();
    }

    /// Exit alternate screen: restore cells/cursor.
    pub fn exit_alt(&mut self) {
        if !self.alt_active { return; }
        std::mem::swap(&mut self.cells, &mut self.alt_cells);
        std::mem::swap(&mut self.cursor, &mut self.alt_cursor);
        self.alt_active = false;
        self.mark_all_dirty();
    }

    /// Resize grid, preserving content where possible.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let new_count = (cols as usize) * (rows as usize);
        let old_cols = self.cols as usize;
        let old_rows = self.rows as usize;
        let mut new_cells = vec![Cell::blank(); new_count];
        let copy_cols = old_cols.min(cols as usize);
        let copy_rows = old_rows.min(rows as usize);
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                new_cells[r * cols as usize + c] = self.cells[r * old_cols + c].clone();
            }
        }
        self.cells = new_cells;
        self.dirty = vec![true; new_count];
        self.cols = cols;
        self.rows = rows;
        self.scroll_top = 0;
        self.scroll_bot = rows.saturating_sub(1);
        self.cursor_move(self.cursor.0, self.cursor.1);

        let alt_count = new_count;
        self.alt_cells = vec![Cell::blank(); alt_count];
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

    #[test]
    fn scroll_up_shifts_rows() {
        let mut g = Grid::new(4, 3);
        // row 0: ABCD, row 1: EFGH, row 2: blank
        for col in 0u16..4 {
            g.write_cell(col, 0, Cell { ch: (b'A' + col as u8) as char, fg: Color::white(), bg: Color::black(), flags: CellFlags::NONE });
            g.write_cell(col, 1, Cell { ch: (b'E' + col as u8) as char, fg: Color::white(), bg: Color::black(), flags: CellFlags::NONE });
        }
        g.scroll_up(1);
        assert_eq!(g.cells[0].ch, 'E');
        assert_eq!(g.cells[4].ch, ' ');
    }

    #[test]
    fn scroll_down_shifts_rows() {
        let mut g = Grid::new(4, 3);
        for col in 0u16..4 {
            g.write_cell(col, 0, Cell { ch: (b'A' + col as u8) as char, fg: Color::white(), bg: Color::black(), flags: CellFlags::NONE });
        }
        g.scroll_down(1);
        // row 0 should be blank, row 1 should have ABCD
        assert_eq!(g.cells[0].ch, ' ');
        assert_eq!(g.cells[4].ch, 'A');
    }

    #[test]
    fn alt_screen_swap() {
        let mut g = Grid::new(4, 2);
        g.write_cell(0, 0, Cell { ch: 'X', fg: Color::white(), bg: Color::black(), flags: CellFlags::NONE });
        g.enter_alt();
        assert_eq!(g.cells[0].ch, ' ');
        assert!(g.alt_active);
        g.exit_alt();
        assert_eq!(g.cells[0].ch, 'X');
        assert!(!g.alt_active);
    }
}
