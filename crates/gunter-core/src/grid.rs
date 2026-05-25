/// Terminal grid types for gunter-terminal.
///
/// These are the canonical public types that `gunter-term` (the VTE performer)
/// and `gunter-renderer` (the wgpu painter) both depend on.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Cell attribute flags (bold, italic, underline, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellFlags(pub u8);

impl CellFlags {
    pub const NONE: CellFlags = CellFlags(0);
    pub const BOLD: CellFlags = CellFlags(1 << 0);
    pub const ITALIC: CellFlags = CellFlags(1 << 1);
    pub const UNDERLINE: CellFlags = CellFlags(1 << 2);

    pub fn contains(self, other: CellFlags) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for CellFlags {
    type Output = CellFlags;
    fn bitor(self, rhs: CellFlags) -> CellFlags {
        CellFlags(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for CellFlags {
    fn bitor_assign(&mut self, rhs: CellFlags) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for CellFlags {
    type Output = CellFlags;
    fn bitand(self, rhs: CellFlags) -> CellFlags {
        CellFlags(self.0 & rhs.0)
    }
}

impl std::ops::Not for CellFlags {
    type Output = CellFlags;
    fn not(self) -> CellFlags {
        CellFlags(!self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl Cell {
    /// Default blank cell with Atom One Dark palette.
    pub fn blank() -> Cell {
        Cell {
            ch: ' ',
            fg: Color { r: 171, g: 178, b: 191 },
            bg: Color { r: 40,  g: 44,  b: 52  },
            flags: CellFlags::NONE,
        }
    }
}

/// The terminal grid: a flat row-major array of `Cell`s plus a cursor.
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub cursor: (u16, u16),
    pub dirty: Vec<bool>,
}

impl Grid {
    pub fn new(cols: u16, rows: u16) -> Grid {
        let n = (cols as usize) * (rows as usize);
        Grid {
            cols,
            rows,
            cells: vec![Cell::blank(); n],
            cursor: (0, 0),
            dirty: vec![false; n],
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

    pub fn cursor_move(&mut self, x: u16, y: u16) {
        self.cursor = (x.min(self.cols.saturating_sub(1)), y.min(self.rows.saturating_sub(1)));
    }

    pub fn clear_dirty(&mut self) {
        for d in &mut self.dirty {
            *d = false;
        }
    }
}
