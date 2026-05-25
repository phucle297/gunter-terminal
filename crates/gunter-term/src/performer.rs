use gunter_core::grid::{Grid, Cell, Color, CellFlags};

/// Default foreground color: Atom One Dark #abb2bf
const DEFAULT_FG: Color = Color { r: 171, g: 178, b: 191 };
/// Default background color: Atom One Dark #282c34
const DEFAULT_BG: Color = Color { r: 40, g: 44, b: 52 };

/// ANSI basic colors for SGR 30-37 / 40-47 (Atom One Dark palette).
const ANSI_COLORS: [Color; 8] = [
    Color { r: 63,  g: 68,  b: 81  },  // 0 black   #3f4451
    Color { r: 224, g: 108, b: 117 },  // 1 red     #e06c75
    Color { r: 152, g: 195, b: 121 },  // 2 green   #98c379
    Color { r: 229, g: 192, b: 123 },  // 3 yellow  #e5c07b
    Color { r: 97,  g: 175, b: 239 },  // 4 blue    #61afef
    Color { r: 198, g: 120, b: 221 },  // 5 magenta #c678dd
    Color { r: 86,  g: 182, b: 194 },  // 6 cyan    #56b6c2
    Color { r: 171, g: 178, b: 191 },  // 7 white   #abb2bf
];

/// VTE performer that writes parsed terminal output into a [`Grid`].
pub struct GridPerformer<'a> {
    pub grid: &'a mut Grid,
    fg: Color,
    bg: Color,
    flags: CellFlags,
}

impl<'a> GridPerformer<'a> {
    pub fn new(grid: &'a mut Grid) -> Self {
        GridPerformer {
            grid,
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            flags: CellFlags::NONE,
        }
    }
}

impl<'a> vte::Perform for GridPerformer<'a> {
    /// Printable character: write to current cursor position and advance right.
    fn print(&mut self, c: char) {
        let (x, y) = self.grid.cursor;
        self.grid.write_cell(x, y, Cell {
            ch: c,
            fg: self.fg,
            bg: self.bg,
            flags: self.flags,
        });
        // Advance cursor, clamping at last column (no wrap for now).
        let next_x = x + 1;
        if next_x < self.grid.cols {
            self.grid.cursor.0 = next_x;
        } else {
            // Stay on last column rather than wrapping off-screen.
            self.grid.cursor.0 = self.grid.cols - 1;
        }
    }

    /// C0/C1 control characters.
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                let y = self.grid.cursor.1;
                let next_y = y + 1;
                if next_y < self.grid.rows {
                    self.grid.cursor.1 = next_y;
                } else {
                    // At bottom — stay (scroll not yet implemented).
                    self.grid.cursor.1 = self.grid.rows - 1;
                }
            }
            b'\r' => {
                self.grid.cursor.0 = 0;
            }
            _ => {}
        }
    }

    /// CSI sequences.
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        match action {
            // Cursor Position: CSI row ; col H  (1-based → 0-based)
            'H' | 'f' => {
                let mut iter = params.iter();
                let row = iter.next().and_then(|s| s.first().copied()).unwrap_or(1);
                let col = iter.next().and_then(|s| s.first().copied()).unwrap_or(1);
                let row = if row == 0 { 0 } else { row - 1 };
                let col = if col == 0 { 0 } else { col - 1 };
                self.grid.cursor_move(col as u16, row as u16);
            }
            // Cursor Up N
            'A' => {
                let n = first_param(params, 1) as u16;
                let y = self.grid.cursor.1;
                self.grid.cursor.1 = y.saturating_sub(n);
            }
            // Cursor Down N
            'B' => {
                let n = first_param(params, 1) as u16;
                let y = self.grid.cursor.1;
                let next = y + n;
                self.grid.cursor.1 = next.min(self.grid.rows - 1);
            }
            // Cursor Right N
            'C' => {
                let n = first_param(params, 1) as u16;
                let x = self.grid.cursor.0;
                let next = x + n;
                self.grid.cursor.0 = next.min(self.grid.cols - 1);
            }
            // Cursor Left N
            'D' => {
                let n = first_param(params, 1) as u16;
                let x = self.grid.cursor.0;
                self.grid.cursor.0 = x.saturating_sub(n);
            }
            // SGR — Select Graphic Rendition
            'm' => {
                // No params at all means SGR 0 (reset).
                let mut iter = params.iter().peekable();
                if iter.peek().is_none() {
                    self.fg = DEFAULT_FG;
                    self.bg = DEFAULT_BG;
                    self.flags = CellFlags::NONE;
                    return;
                }
                for sub in iter {
                    let code = sub.first().copied().unwrap_or(0);
                    match code {
                        0 => {
                            self.fg = DEFAULT_FG;
                            self.bg = DEFAULT_BG;
                            self.flags = CellFlags::NONE;
                        }
                        1 => self.flags |= CellFlags::BOLD,
                        3 => self.flags |= CellFlags::ITALIC,
                        4 => self.flags |= CellFlags::UNDERLINE,
                        30..=37 => self.fg = ANSI_COLORS[(code - 30) as usize],
                        39 => self.fg = DEFAULT_FG,
                        40..=47 => self.bg = ANSI_COLORS[(code - 40) as usize],
                        49 => self.bg = DEFAULT_BG,
                        _ => {} // unhandled — ignored per spec
                    }
                }
            }
            _ => {}
        }
    }
}

/// Extract the first scalar param, returning `default` when absent or zero.
fn first_param(params: &vte::Params, default: u16) -> u16 {
    let v = params.iter().next().and_then(|s| s.first().copied()).unwrap_or(0);
    if v == 0 { default } else { v as u16 }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_grid() -> Grid {
        Grid::new(80, 24)
    }

    #[test]
    fn print_writes_char_to_cursor() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"A" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].ch, 'A');
    }

    #[test]
    fn print_advances_cursor_right() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"AB" { parser.advance(&mut p, *b); }
        // 'A' at col 0, 'B' at col 1, cursor now at col 2
        assert_eq!(grid.cells[0].ch, 'A');
        assert_eq!(grid.cells[1].ch, 'B');
        assert_eq!(grid.cursor.0, 2);
    }

    #[test]
    fn newline_moves_cursor_down() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\n" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 1);
    }

    #[test]
    fn carriage_return_moves_to_col_zero() {
        let mut grid = make_grid();
        grid.cursor_move(10, 0);
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\r" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.0, 0);
    }

    #[test]
    fn csi_cursor_position_moves_cursor() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        // CSI 5;10H → row=5, col=10 (1-based) → cursor=(9,4)
        for b in b"\x1b[5;10H" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor, (9, 4));
    }

    #[test]
    fn csi_cursor_position_default_is_home() {
        let mut grid = make_grid();
        grid.cursor_move(5, 5);
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        // CSI H with no params → (0,0)
        for b in b"\x1b[H" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor, (0, 0));
    }

    #[test]
    fn csi_cursor_up() {
        let mut grid = make_grid();
        grid.cursor_move(0, 5);
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[3A" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 2);
    }

    #[test]
    fn csi_cursor_down() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[2B" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 2);
    }

    #[test]
    fn csi_cursor_right() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[5C" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.0, 5);
    }

    #[test]
    fn csi_cursor_left() {
        let mut grid = make_grid();
        grid.cursor_move(10, 0);
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[4D" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.0, 6);
    }

    #[test]
    fn sgr_sets_foreground_color() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        // SGR 31 = red fg, then print 'X'
        for b in b"\x1b[31mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, Color { r: 224, g: 108, b: 117 });
    }

    #[test]
    fn sgr_sets_background_color() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        // SGR 42 = green bg, then print 'X'
        for b in b"\x1b[42mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].bg, Color { r: 152, g: 195, b: 121 });
    }

    #[test]
    fn sgr_reset_restores_defaults() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        // Set red fg, then reset, then print
        for b in b"\x1b[31m\x1b[0mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, DEFAULT_FG);
        assert_eq!(grid.cells[0].bg, DEFAULT_BG);
    }

    #[test]
    fn sgr_bold() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[1mX" { parser.advance(&mut p, *b); }
        assert!(grid.cells[0].flags.contains(CellFlags::BOLD));
    }

    #[test]
    fn sgr_italic() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[3mX" { parser.advance(&mut p, *b); }
        assert!(grid.cells[0].flags.contains(CellFlags::ITALIC));
    }

    #[test]
    fn sgr_underline() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[4mX" { parser.advance(&mut p, *b); }
        assert!(grid.cells[0].flags.contains(CellFlags::UNDERLINE));
    }

    #[test]
    fn sgr_no_params_is_reset() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        // SGR 31, then bare ESC[m (no param = reset)
        for b in b"\x1b[31m\x1b[mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, DEFAULT_FG);
    }

    #[test]
    fn write_cell_marks_dirty() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"Z" { parser.advance(&mut p, *b); }
        assert!(grid.dirty[0]);
    }

    #[test]
    fn cursor_clamps_at_bottom_on_newline() {
        let mut grid = make_grid();
        grid.cursor_move(0, 23); // last row
        let mut p = GridPerformer::new(&mut grid);
        let mut parser = vte::Parser::new();
        for b in b"\n" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 23); // stays at last row
    }
}
