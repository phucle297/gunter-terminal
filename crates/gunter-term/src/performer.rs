use gunter_core::grid::{Grid, Cell, Color, CellFlags, TermColor};

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

/// Bright colors for SGR 90-97 / 100-107 (Atom One Dark bright palette).
const BRIGHT_COLORS: [Color; 8] = [
    Color { r: 79,  g: 86,  b: 102 },  // bright black  #4f5666
    Color { r: 224, g: 108, b: 117 },  // bright red    #e06c75
    Color { r: 152, g: 195, b: 121 },  // bright green  #98c379
    Color { r: 229, g: 192, b: 123 },  // bright yellow #e5c07b
    Color { r: 97,  g: 175, b: 239 },  // bright blue   #61afef
    Color { r: 198, g: 120, b: 221 },  // bright magenta #c678dd
    Color { r: 86,  g: 182, b: 194 },  // bright cyan   #56b6c2
    Color { r: 255, g: 255, b: 255 },  // bright white  #ffffff
];

/// VTE performer that writes parsed terminal output into a [`Grid`].
pub struct GridPerformer<'a> {
    pub grid: &'a mut Grid,
    pub response_tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    fg: TermColor,
    bg: TermColor,
    flags: CellFlags,
}

impl<'a> GridPerformer<'a> {
    pub fn new(grid: &'a mut Grid, response_tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>) -> Self {
        GridPerformer {
            grid,
            response_tx,
            fg: TermColor::Default,
            bg: TermColor::Default,
            flags: CellFlags::NONE,
        }
    }

    fn respond(&self, bytes: &[u8]) {
        if let Some(tx) = &self.response_tx {
            let _ = tx.try_send(bytes.to_vec());
        }
    }

    fn blank(&self) -> Cell {
        Cell { ch: ' ', fg: self.fg, bg: self.bg, flags: CellFlags::NONE }
    }

    fn advance_line(&mut self) {
        let y = self.grid.cursor.1;
        let bot = self.grid.scroll_bot;
        if y >= bot {
            self.grid.scroll_up(1);
        } else {
            self.grid.cursor.1 = y + 1;
        }
    }
}

impl<'a> vte::Perform for GridPerformer<'a> {
    /// Printable character: write to current cursor position and advance right.
    fn print(&mut self, c: char) {
        // If deferred wrap is pending, execute it now before printing
        if self.grid.wrap_next {
            self.grid.wrap_next = false;
            self.grid.cursor.0 = 0;
            self.advance_line();
        }
        let (x, y) = self.grid.cursor;
        self.grid.write_cell(x, y, Cell { ch: c, fg: self.fg, bg: self.bg, flags: self.flags });
        let next_x = x + 1;
        if next_x < self.grid.cols {
            self.grid.cursor.0 = next_x;
        } else if self.grid.auto_wrap {
            // Cursor is at last col — set deferred wrap, don't move cursor yet
            self.grid.wrap_next = true;
        }
        // if auto_wrap disabled, cursor stays at last col, no wrap_next
    }

    /// C0/C1 control characters.
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | b'\x0b' | b'\x0c' => {
                self.advance_line();
            }
            b'\r' => {
                self.grid.cursor.0 = 0;
                self.grid.wrap_next = false;
            }
            b'\x08' => {
                let x = self.grid.cursor.0;
                self.grid.cursor.0 = x.saturating_sub(1);
            }
            b'\x07' => {} // BEL — ignore
            _ => {}
        }
    }

    /// CSI sequences.
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
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
                self.grid.wrap_next = false;
            }
            // Cursor Down N
            'B' => {
                let n = first_param(params, 1) as u16;
                let y = self.grid.cursor.1;
                let next = y + n;
                self.grid.cursor.1 = next.min(self.grid.rows - 1);
                self.grid.wrap_next = false;
            }
            // Cursor Right N
            'C' => {
                let n = first_param(params, 1) as u16;
                let x = self.grid.cursor.0;
                let next = x + n;
                self.grid.cursor.0 = next.min(self.grid.cols - 1);
                self.grid.wrap_next = false;
            }
            // Cursor Left N
            'D' => {
                let n = first_param(params, 1) as u16;
                let x = self.grid.cursor.0;
                self.grid.cursor.0 = x.saturating_sub(n);
                self.grid.wrap_next = false;
            }
            // Cursor Next Line N
            'E' => {
                let n = first_param(params, 1) as u16;
                let y = self.grid.cursor.1;
                let next = y + n;
                self.grid.cursor.1 = next.min(self.grid.rows - 1);
                self.grid.cursor.0 = 0;
                self.grid.wrap_next = false;
            }
            // Cursor Preceding Line N
            'F' => {
                let n = first_param(params, 1) as u16;
                let y = self.grid.cursor.1;
                self.grid.cursor.1 = y.saturating_sub(n);
                self.grid.cursor.0 = 0;
                self.grid.wrap_next = false;
            }
            // Cursor Column Absolute (1-based)
            'G' => {
                let col = first_param(params, 1) as u16;
                let col = col.saturating_sub(1);
                self.grid.cursor.0 = col.min(self.grid.cols - 1);
                self.grid.wrap_next = false;
            }
            // Line Position Absolute (1-based)
            'd' => {
                let row = first_param(params, 1) as u16;
                let row = row.saturating_sub(1);
                self.grid.cursor.1 = row.min(self.grid.rows - 1);
                self.grid.wrap_next = false;
            }
            // Erase in Display
            'J' => {
                let param = first_param_raw(params, 0);
                let (cx, cy) = self.grid.cursor;
                let cols = self.grid.cols;
                let rows = self.grid.rows;
                let blank = self.blank();
                match param {
                    0 => {
                        for x in cx..cols { self.grid.write_cell(x, cy, blank); }
                        for y in (cy + 1)..rows {
                            for x in 0..cols { self.grid.write_cell(x, y, blank); }
                        }
                    }
                    1 => {
                        for y in 0..cy {
                            for x in 0..cols { self.grid.write_cell(x, y, blank); }
                        }
                        for x in 0..=cx { self.grid.write_cell(x, cy, blank); }
                    }
                    2 | 3 => {
                        for y in 0..rows {
                            for x in 0..cols { self.grid.write_cell(x, y, blank); }
                        }
                        self.grid.cursor = (0, 0);
                        self.grid.wrap_next = false;
                    }
                    _ => {}
                }
            }
            // Erase in Line
            'K' => {
                let param = first_param_raw(params, 0);
                let (cx, cy) = self.grid.cursor;
                let cols = self.grid.cols;
                let blank = self.blank();
                match param {
                    0 => {
                        for x in cx..cols { self.grid.write_cell(x, cy, blank); }
                    }
                    1 => {
                        for x in 0..=cx { self.grid.write_cell(x, cy, blank); }
                    }
                    2 => {
                        for x in 0..cols { self.grid.write_cell(x, cy, blank); }
                    }
                    _ => {}
                }
            }
            // DECSTBM — Set Scroll Region
            'r' => {
                let mut iter = params.iter();
                let top = iter.next().and_then(|s| s.first().copied()).unwrap_or(1);
                let bot = iter.next().and_then(|s| s.first().copied()).unwrap_or(self.grid.rows);
                let top = if top == 0 { 0 } else { (top - 1) as u16 };
                let bot = if bot == 0 { self.grid.rows - 1 } else { (bot - 1) as u16 };
                self.grid.scroll_top = top.min(self.grid.rows - 1);
                self.grid.scroll_bot = bot.min(self.grid.rows - 1);
                self.grid.cursor = (0, 0);
            }
            // Scroll Up N lines
            'S' => {
                let n = first_param(params, 1);
                self.grid.scroll_up(n);
            }
            // Scroll Down N lines
            'T' => {
                let n = first_param(params, 1);
                self.grid.scroll_down(n);
            }
            // Insert N Lines at cursor row
            'L' => {
                let n = first_param(params, 1);
                let saved_top = self.grid.scroll_top;
                self.grid.scroll_top = self.grid.cursor.1;
                self.grid.scroll_down(n);
                self.grid.scroll_top = saved_top;
            }
            // Delete N Lines at cursor row
            'M' => {
                let n = first_param(params, 1);
                let saved_top = self.grid.scroll_top;
                self.grid.scroll_top = self.grid.cursor.1;
                self.grid.scroll_up(n);
                self.grid.scroll_top = saved_top;
            }
            // Insert N Characters (shift right)
            '@' => {
                let n = first_param(params, 1) as usize;
                let cx = self.grid.cursor.0 as usize;
                let cy = self.grid.cursor.1;
                let cols = self.grid.cols as usize;
                let blank = self.blank();
                let saved: Vec<Cell> = (cx..cols.saturating_sub(n))
                    .map(|c| self.grid.cells[cy as usize * cols + c])
                    .collect();
                let end = cols.min(cx + n);
                for c in cx..end {
                    self.grid.write_cell(c as u16, cy, blank);
                }
                for (i, cell) in saved.into_iter().enumerate() {
                    let dst = cx + n + i;
                    if dst < cols { self.grid.write_cell(dst as u16, cy, cell); }
                }
            }
            // Delete N Characters (shift left)
            'P' => {
                let n = first_param(params, 1) as usize;
                let cx = self.grid.cursor.0 as usize;
                let cy = self.grid.cursor.1;
                let cols = self.grid.cols as usize;
                let blank = self.blank();
                for c in cx..cols {
                    let src_c = c + n;
                    let cell = if src_c < cols {
                        self.grid.cells[cy as usize * cols + src_c]
                    } else {
                        blank
                    };
                    self.grid.write_cell(c as u16, cy, cell);
                }
            }
            // Erase N Characters (no cursor move)
            'X' => {
                let n = first_param(params, 1) as u16;
                let (cx, cy) = self.grid.cursor;
                let blank = self.blank();
                for i in 0..n {
                    let x = cx + i;
                    if x < self.grid.cols {
                        self.grid.write_cell(x, cy, blank);
                    }
                }
            }
            // SGR — Select Graphic Rendition
            'm' => {
                let mut iter = params.iter().peekable();
                if iter.peek().is_none() {
                    self.fg = TermColor::Default;
                    self.bg = TermColor::Default;
                    self.flags = CellFlags::NONE;
                    return;
                }
                let codes: Vec<u16> = params.iter()
                    .flat_map(|sub| sub.iter().copied())
                    .collect();
                let mut i = 0;
                while i < codes.len() {
                    match codes[i] {
                        0 => { self.fg = TermColor::Default; self.bg = TermColor::Default; self.flags = CellFlags::NONE; }
                        1 => self.flags |= CellFlags::BOLD,
                        3 => self.flags |= CellFlags::ITALIC,
                        4 => self.flags |= CellFlags::UNDERLINE,
                        5 => self.flags |= CellFlags::BLINK,
                        7 => self.flags |= CellFlags::INVERSE,
                        22 => self.flags &= !CellFlags::BOLD,
                        23 => self.flags &= !CellFlags::ITALIC,
                        24 => self.flags &= !CellFlags::UNDERLINE,
                        27 => self.flags &= !CellFlags::INVERSE,
                        30..=37 => {
                            let c = ANSI_COLORS[(codes[i] - 30) as usize];
                            self.fg = TermColor::Rgb(c.r, c.g, c.b);
                        }
                        38 => {
                            if i + 2 < codes.len() && codes[i+1] == 5 {
                                self.fg = TermColor::Indexed(codes[i+2] as u8);
                                i += 2;
                            } else if i + 4 < codes.len() && codes[i+1] == 2 {
                                self.fg = TermColor::Rgb(codes[i+2] as u8, codes[i+3] as u8, codes[i+4] as u8);
                                i += 4;
                            }
                        }
                        39 => self.fg = TermColor::Default,
                        40..=47 => {
                            let c = ANSI_COLORS[(codes[i] - 40) as usize];
                            self.bg = TermColor::Rgb(c.r, c.g, c.b);
                        }
                        48 => {
                            if i + 2 < codes.len() && codes[i+1] == 5 {
                                self.bg = TermColor::Indexed(codes[i+2] as u8);
                                i += 2;
                            } else if i + 4 < codes.len() && codes[i+1] == 2 {
                                self.bg = TermColor::Rgb(codes[i+2] as u8, codes[i+3] as u8, codes[i+4] as u8);
                                i += 4;
                            }
                        }
                        49 => self.bg = TermColor::Default,
                        90..=97 => {
                            let c = BRIGHT_COLORS[(codes[i] - 90) as usize];
                            self.fg = TermColor::Rgb(c.r, c.g, c.b);
                        }
                        100..=107 => {
                            let c = BRIGHT_COLORS[(codes[i] - 100) as usize];
                            self.bg = TermColor::Rgb(c.r, c.g, c.b);
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            // Private mode set
            'h' if intermediates == b"?" => {
                for sub in params.iter() {
                    match sub.first().copied().unwrap_or(0) {
                        1 => self.grid.app_cursor_keys = true,
                        7 => self.grid.auto_wrap = true,
                        25 => self.grid.cursor_visible = true,
                        1000 | 1002 => self.grid.mouse_reporting = true,
                        1006 => { self.grid.mouse_reporting = true; self.grid.mouse_sgr = true; }
                        2004 => self.grid.bracketed_paste = true,
                        1049 => self.grid.enter_alt(),
                        _ => {}
                    }
                }
            }
            // Private mode reset
            'l' if intermediates == b"?" => {
                for sub in params.iter() {
                    match sub.first().copied().unwrap_or(0) {
                        1 => self.grid.app_cursor_keys = false,
                        7 => self.grid.auto_wrap = false,
                        25 => self.grid.cursor_visible = false,
                        1000 | 1002 => self.grid.mouse_reporting = false,
                        1006 => { self.grid.mouse_reporting = false; self.grid.mouse_sgr = false; }
                        2004 => self.grid.bracketed_paste = false,
                        1049 => self.grid.exit_alt(),
                        _ => {}
                    }
                }
            }
            // CSI s — save cursor (alias for ESC 7)
            's' if intermediates.is_empty() => {
                use gunter_core::grid::SavedCursor;
                let (x, y) = self.grid.cursor;
                self.grid.saved_cursor = Some(SavedCursor {
                    x, y,
                    fg: self.fg,
                    bg: self.bg,
                    flags: self.flags,
                });
            }
            // CSI u — restore cursor (alias for ESC 8)
            'u' if intermediates.is_empty() => {
                if let Some(sc) = self.grid.saved_cursor {
                    self.grid.cursor_move(sc.x, sc.y);
                    self.fg = sc.fg;
                    self.bg = sc.bg;
                    self.flags = sc.flags;
                }
            }
            // P1.3: Primary Device Attributes (DA1)
            'c' if intermediates.is_empty() => {
                let param = first_param_raw(params, 0);
                if param == 0 {
                    self.respond(b"\x1b[?62;c");
                }
            }
            // P1.4: Device Status Report (CPR)
            'n' if intermediates.is_empty() => {
                let param = first_param_raw(params, 0);
                if param == 6 {
                    let (col, row) = self.grid.cursor;
                    let response = format!("\x1b[{};{}R", row + 1, col + 1);
                    self.respond(response.as_bytes());
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates, byte) {
            // ESC M — reverse index
            (b"", b'M') => {
                let y = self.grid.cursor.1;
                if y == self.grid.scroll_top {
                    self.grid.scroll_down(1);
                } else {
                    self.grid.cursor.1 = y.saturating_sub(1);
                }
            }
            // ESC 7 — save cursor
            (b"", b'7') => {
                use gunter_core::grid::SavedCursor;
                let (x, y) = self.grid.cursor;
                self.grid.saved_cursor = Some(SavedCursor {
                    x, y,
                    fg: self.fg,
                    bg: self.bg,
                    flags: self.flags,
                });
            }
            // ESC 8 — restore cursor
            (b"", b'8') => {
                if let Some(sc) = self.grid.saved_cursor {
                    self.grid.cursor_move(sc.x, sc.y);
                    self.fg = sc.fg;
                    self.bg = sc.bg;
                    self.flags = sc.flags;
                }
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 or OSC 2: set window title
        if params.len() >= 2 {
            let code = params[0];
            if code == b"0" || code == b"2" {
                if let Ok(title) = std::str::from_utf8(params[1]) {
                    self.grid.title = title.to_string();
                }
            }
        }
    }
}

/// Extract the first scalar param, returning `default` when absent or zero.
fn first_param(params: &vte::Params, default: u16) -> u16 {
    let v = params.iter().next().and_then(|s| s.first().copied()).unwrap_or(0);
    if v == 0 { default } else { v as u16 }
}

/// Extract first scalar param raw (zero is a valid value here).
fn first_param_raw(params: &vte::Params, default: u16) -> u16 {
    params.iter().next().and_then(|s| s.first().copied()).unwrap_or(default) as u16
}

fn xterm256(n: u16) -> Color {
    match n {
        0..=7 => ANSI_COLORS[n as usize],
        8..=15 => BRIGHT_COLORS[(n - 8) as usize],
        16..=231 => {
            let n = n - 16;
            let b = n % 6;
            let g = (n / 6) % 6;
            let r = n / 36;
            let cv = |v: u16| -> u8 { if v == 0 { 0 } else { (55 + v * 40) as u8 } };
            Color { r: cv(r), g: cv(g), b: cv(b) }
        }
        232..=255 => {
            let v = (8 + (n - 232) * 10) as u8;
            Color { r: v, g: v, b: v }
        }
        _ => Color { r: 0, g: 0, b: 0 }
    }
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
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"A" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].ch, 'A');
    }

    #[test]
    fn print_advances_cursor_right() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
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
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\n" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 1);
    }

    #[test]
    fn carriage_return_moves_to_col_zero() {
        let mut grid = make_grid();
        grid.cursor_move(10, 0);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\r" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.0, 0);
    }

    #[test]
    fn csi_cursor_position_moves_cursor() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // CSI 5;10H → row=5, col=10 (1-based) → cursor=(9,4)
        for b in b"\x1b[5;10H" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor, (9, 4));
    }

    #[test]
    fn csi_cursor_position_default_is_home() {
        let mut grid = make_grid();
        grid.cursor_move(5, 5);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // CSI H with no params → (0,0)
        for b in b"\x1b[H" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor, (0, 0));
    }

    #[test]
    fn csi_cursor_up() {
        let mut grid = make_grid();
        grid.cursor_move(0, 5);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[3A" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 2);
    }

    #[test]
    fn csi_cursor_down() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[2B" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 2);
    }

    #[test]
    fn csi_cursor_right() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[5C" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.0, 5);
    }

    #[test]
    fn csi_cursor_left() {
        let mut grid = make_grid();
        grid.cursor_move(10, 0);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[4D" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.0, 6);
    }

    #[test]
    fn sgr_sets_foreground_color() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // SGR 31 = red fg, then print 'X'
        for b in b"\x1b[31mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, TermColor::Rgb(224, 108, 117));
    }

    #[test]
    fn sgr_sets_background_color() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // SGR 42 = green bg, then print 'X'
        for b in b"\x1b[42mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].bg, TermColor::Rgb(152, 195, 121));
    }

    #[test]
    fn sgr_reset_restores_defaults() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // Set red fg, then reset, then print
        for b in b"\x1b[31m\x1b[0mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, TermColor::Default);
        assert_eq!(grid.cells[0].bg, TermColor::Default);
    }

    #[test]
    fn sgr_bold() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[1mX" { parser.advance(&mut p, *b); }
        assert!(grid.cells[0].flags.contains(CellFlags::BOLD));
    }

    #[test]
    fn sgr_italic() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[3mX" { parser.advance(&mut p, *b); }
        assert!(grid.cells[0].flags.contains(CellFlags::ITALIC));
    }

    #[test]
    fn sgr_underline() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[4mX" { parser.advance(&mut p, *b); }
        assert!(grid.cells[0].flags.contains(CellFlags::UNDERLINE));
    }

    #[test]
    fn sgr_no_params_is_reset() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // SGR 31, then bare ESC[m (no param = reset)
        for b in b"\x1b[31m\x1b[mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, TermColor::Default);
    }

    #[test]
    fn write_cell_marks_dirty() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"Z" { parser.advance(&mut p, *b); }
        assert!(grid.dirty[0]);
    }

    #[test]
    fn newline_at_bottom_scrolls() {
        let mut grid = make_grid();
        // write 'A' at row 0 col 0, move to last row, then newline → scroll
        grid.write_cell(0, 0, Cell { ch: 'A', fg: TermColor::Default, bg: TermColor::Default, flags: CellFlags::NONE });
        grid.cursor_move(0, 23);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\n" { parser.advance(&mut p, *b); }
        // cursor stays on last row, content scrolled up
        assert_eq!(grid.cursor.1, 23);
        // row 0 scrolled out: first cell is what was row 1 (blank)
        assert_eq!(grid.cells[0].ch, ' ');
    }

    #[test]
    fn print_wraps_at_end_of_line() {
        let mut grid = Grid::new(4, 3);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // 5 chars on a 4-col grid: should wrap
        for b in b"ABCDE" { parser.advance(&mut p, *b); }
        // row 0: ABCD, row 1: E
        assert_eq!(grid.cells[0].ch, 'A');
        assert_eq!(grid.cells[3].ch, 'D');
        assert_eq!(grid.cells[4].ch, 'E');
        assert_eq!(grid.cursor, (1, 1));
    }

    #[test]
    fn erase_to_end_of_line() {
        let mut grid = make_grid();
        // write 'X' at cols 0-4, then move to col 2 and CSI 0K
        for b in b"XXXXX" { vte::Parser::new().advance(&mut GridPerformer::new(&mut grid, None), *b); }
        grid.cursor_move(2, 0);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[0K" { parser.advance(&mut p, *b); }
        // cols 0,1 should still be 'X'; cols 2+ should be ' '
        assert_eq!(grid.cells[0].ch, 'X');
        assert_eq!(grid.cells[1].ch, 'X');
        assert_eq!(grid.cells[2].ch, ' ');
        assert_eq!(grid.cells[3].ch, ' ');
    }

    #[test]
    fn erase_entire_line() {
        let mut grid = make_grid();
        // write 'X' across row 0
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            for b in b"XXXXX" { parser.advance(&mut p, *b); }
        }
        grid.cursor_move(0, 0);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[2K" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].ch, ' ');
        assert_eq!(grid.cells[4].ch, ' ');
    }

    #[test]
    fn erase_to_end_of_display() {
        let mut grid = make_grid();
        // write 'X' on row 0 and row 1
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            for b in b"XX\nXX" { parser.advance(&mut p, *b); }
        }
        // move to row 1 col 0, erase to end of display
        grid.cursor_move(0, 1);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[0J" { parser.advance(&mut p, *b); }
        // row 0 should still have 'X' at cols 0,1
        assert_eq!(grid.cells[0].ch, 'X');
        assert_eq!(grid.cells[1].ch, 'X');
        // row 1 onward should be blank
        assert_eq!(grid.cells[80].ch, ' ');
        assert_eq!(grid.cells[81].ch, ' ');
    }

    #[test]
    fn scroll_region_set() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // CSI 2;5r → scroll_top=1 (0-based), scroll_bot=4 (0-based)
        for b in b"\x1b[2;5r" { parser.advance(&mut p, *b); }
        assert_eq!(grid.scroll_top, 1);
        assert_eq!(grid.scroll_bot, 4);
    }

    #[test]
    fn sgr_bright_fg() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // SGR 91 = bright red fg
        for b in b"\x1b[91mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, TermColor::Rgb(224, 108, 117));
    }

    #[test]
    fn sgr_256_fg() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // SGR 38;5;196 → index 196 stored as Indexed(196)
        for b in b"\x1b[38;5;196mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, TermColor::Indexed(196));
    }

    #[test]
    fn sgr_truecolor_fg() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[38;2;100;200;50mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].fg, TermColor::Rgb(100, 200, 50));
    }

    #[test]
    fn sgr_truecolor_bg() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[48;2;10;20;30mX" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cells[0].bg, TermColor::Rgb(10, 20, 30));
    }

    #[test]
    fn wrap_next_deferred_wrap() {
        let mut grid = Grid::new(4, 3);
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            for b in b"ABCD" { parser.advance(&mut p, *b); }
        }
        assert_eq!(grid.cursor.0, 3, "cursor should stay at last col");
        assert!(grid.wrap_next, "wrap_next should be set");
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            for b in b"E" { parser.advance(&mut p, *b); }
        }
        assert_eq!(grid.cells[4].ch, 'E', "E should be at row 1 col 0");
        assert_eq!(grid.cursor, (1, 1), "cursor should be at col 1, row 1");
        assert!(!grid.wrap_next, "wrap_next should be cleared");
    }

    #[test]
    fn wrap_next_cleared_by_cursor_move() {
        let mut grid = Grid::new(4, 3);
        grid.wrap_next = true;
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            for b in b"\x1b[H" { parser.advance(&mut p, *b); }
        }
        assert!(!grid.wrap_next, "cursor move should clear wrap_next");
    }

    #[test]
    fn osc_window_title_set() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b]2;Hello\x07" { parser.advance(&mut p, *b); }
        assert_eq!(grid.title, "Hello");
    }

    #[test]
    fn osc_icon_and_title_set() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b]0;MyTerm\x07" { parser.advance(&mut p, *b); }
        assert_eq!(grid.title, "MyTerm");
    }

    #[test]
    fn osc_unknown_code_ignored() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b]9;some notification\x07" { parser.advance(&mut p, *b); }
        assert_eq!(grid.title, "");
    }

    #[test]
    fn esc_reverse_index_moves_cursor_up() {
        let mut grid = Grid::new(80, 24);
        grid.cursor_move(0, 5);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1bM" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 4, "cursor should move up one row");
    }

    #[test]
    fn esc_reverse_index_at_top_scrolls_down() {
        let mut grid = Grid::new(4, 4);
        grid.write_cell(0, 0, gunter_core::grid::Cell {
            ch: 'X',
            fg: gunter_core::grid::TermColor::Rgb(255, 255, 255),
            bg: gunter_core::grid::TermColor::Rgb(0, 0, 0),
            flags: gunter_core::grid::CellFlags::NONE,
        });
        grid.cursor_move(0, 0);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1bM" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor.1, 0, "cursor stays at top");
        assert_eq!(grid.cells[0].ch, ' ', "row 0 col 0 should be blank after scroll down");
        assert_eq!(grid.cells[4].ch, 'X', "old row 0 content should be at row 1");
    }

    #[test]
    fn esc_save_restore_cursor() {
        let mut grid = Grid::new(80, 24);
        grid.cursor_move(10, 5);
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            for b in b"\x1b7" { parser.advance(&mut p, *b); }
            for b in b"\x1b[H" { parser.advance(&mut p, *b); }
        }
        assert_eq!(grid.cursor, (0, 0));
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            for b in b"\x1b8" { parser.advance(&mut p, *b); }
        }
        assert_eq!(grid.cursor, (10, 5), "cursor should be restored");
    }

    #[test]
    fn csi_save_restore_cursor_alias() {
        let mut grid = Grid::new(80, 24);
        grid.cursor_move(3, 7);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[s" { parser.advance(&mut p, *b); }
        for b in b"\x1b[H" { parser.advance(&mut p, *b); }
        for b in b"\x1b[u" { parser.advance(&mut p, *b); }
        assert_eq!(grid.cursor, (3, 7));
    }

    #[test]
    fn alternate_screen_enter_exit() {
        let mut grid = make_grid();
        // write sentinel on primary screen
        grid.write_cell(0, 0, Cell { ch: 'Z', fg: TermColor::Default, bg: TermColor::Default, flags: CellFlags::NONE });
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            // enter alt screen
            for b in b"\x1b[?1049h" { parser.advance(&mut p, *b); }
        }
        assert!(grid.alt_active);
        assert_eq!(grid.cells[0].ch, ' '); // alt screen is blank
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            // exit alt screen
            for b in b"\x1b[?1049l" { parser.advance(&mut p, *b); }
        }
        assert!(!grid.alt_active);
        assert_eq!(grid.cells[0].ch, 'Z'); // primary restored
    }

    #[test]
    fn response_tx_is_stored() {
        let mut grid = make_grid();
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(4);
        let p = GridPerformer::new(&mut grid, Some(tx));
        assert!(p.response_tx.is_some());
        drop(p);
        drop(rx);
    }

    #[test]
    fn respond_sends_to_channel() {
        let mut grid = make_grid();
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(4);
        {
            let p = GridPerformer::new(&mut grid, Some(tx));
            p.respond(b"hello");
        }
        let received = rx.try_recv().unwrap();
        assert_eq!(received, b"hello");
    }

    // P1.3: DA1 — Primary Device Attributes
    #[test]
    fn da1_response_sent() {
        use std::sync::mpsc;
        let mut grid = Grid::new(80, 24);
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(4);
        {
            let mut p = GridPerformer::new(&mut grid, Some(tx));
            let mut parser = vte::Parser::new();
            // ESC[c → DA1 query
            for b in b"\x1b[c" { parser.advance(&mut p, *b); }
        }
        let response = rx.try_recv().expect("should have sent DA1 response");
        assert_eq!(response, b"\x1b[?62;c");
    }

    // P1.4: CPR — Cursor Position Report
    #[test]
    fn cpr_response_sent() {
        use std::sync::mpsc;
        let mut grid = Grid::new(80, 24);
        grid.cursor_move(4, 9); // col=4, row=9
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(4);
        {
            let mut p = GridPerformer::new(&mut grid, Some(tx));
            let mut parser = vte::Parser::new();
            // ESC[6n → CPR query
            for b in b"\x1b[6n" { parser.advance(&mut p, *b); }
        }
        let response = rx.try_recv().expect("should have sent CPR response");
        // row=9 → row+1=10, col=4 → col+1=5
        assert_eq!(response, b"\x1b[10;5R");
    }

    // P1.7: Application Cursor Keys ?1h/l
    #[test]
    fn app_cursor_keys_mode_set() {
        let mut grid = Grid::new(80, 24);
        assert!(!grid.app_cursor_keys, "default should be off");
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            // ESC[?1h → enable app cursor keys
            for b in b"\x1b[?1h" { parser.advance(&mut p, *b); }
        }
        assert!(grid.app_cursor_keys, "app cursor keys should be enabled");
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            // ESC[?1l → disable
            for b in b"\x1b[?1l" { parser.advance(&mut p, *b); }
        }
        assert!(!grid.app_cursor_keys, "app cursor keys should be disabled");
    }

    // P1.8: DECAWM Auto-Wrap Mode ?7h/l
    #[test]
    fn decawm_mode_set() {
        let mut grid = Grid::new(80, 24);
        assert!(grid.auto_wrap, "default should be on");
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            // ESC[?7l → disable auto-wrap
            for b in b"\x1b[?7l" { parser.advance(&mut p, *b); }
        }
        assert!(!grid.auto_wrap);
        {
            let mut p = GridPerformer::new(&mut grid, None);
            let mut parser = vte::Parser::new();
            // ESC[?7h → re-enable
            for b in b"\x1b[?7h" { parser.advance(&mut p, *b); }
        }
        assert!(grid.auto_wrap);
    }

    #[test]
    fn sgr_underline_flag_set() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[4mX" { parser.advance(&mut p, *b); }
        assert!(grid.cells[0].flags.contains(CellFlags::UNDERLINE));
    }

    #[test]
    fn sgr_inverse_flag_set() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[7mX" { parser.advance(&mut p, *b); }
        assert!(grid.cells[0].flags.contains(CellFlags::INVERSE));
    }

    #[test]
    fn sgr_27_clears_inverse() {
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[7m\x1b[27mX" { parser.advance(&mut p, *b); }
        assert!(!grid.cells[0].flags.contains(CellFlags::INVERSE));
    }

    #[test]
    fn sgr_default_fg_is_termcolor_default() {
        use gunter_core::grid::TermColor;
        let mut grid = make_grid();
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        for b in b"\x1b[31m\x1b[39mX" { parser.advance(&mut p, *b); }
        // After SGR 31 (red fg) then SGR 39 (reset fg), fg should be Default
        assert_eq!(grid.cells[0].fg, TermColor::Default);
    }

    #[test]
    fn decawm_disabled_cursor_sticks_at_last_col() {
        // With auto_wrap=false, printing past end of line should NOT wrap
        let mut grid = Grid::new(4, 3);
        let mut p = GridPerformer::new(&mut grid, None);
        let mut parser = vte::Parser::new();
        // Disable auto-wrap
        for b in b"\x1b[?7l" { parser.advance(&mut p, *b); }
        // Print 6 chars on 4-col grid
        for b in b"ABCDEF" { parser.advance(&mut p, *b); }
        // Cursor should be at last col (3), no wrap, E and F overwrite col 3
        assert_eq!(grid.cursor.0, 3, "cursor should stick at last col");
        assert_eq!(grid.cursor.1, 0, "cursor should stay on row 0");
        assert!(!grid.wrap_next, "wrap_next should not be set when auto_wrap disabled");
        // Col 3 should have the LAST char written (F)
        assert_eq!(grid.cells[3].ch, 'F', "last char should overwrite col 3");
    }
}
