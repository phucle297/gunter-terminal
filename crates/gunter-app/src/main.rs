use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use gunter_core::grid::Grid;
use gunter_term::performer::GridPerformer;
use std::io::Read;

fn print_grid(grid: &Grid) {
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let idx = (row as usize) * (grid.cols as usize) + (col as usize);
            print!("{}", grid.cells[idx].ch);
        }
        println!();
    }
}

fn main() {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .expect("openpty failed");

    let cmd = CommandBuilder::new("wsl.exe");
    let _child = pair.slave.spawn_command(cmd).expect("spawn failed");

    let mut reader = pair.master.try_clone_reader().expect("clone reader failed");
    let mut parser = vte::Parser::new();
    let mut grid = Grid::new(80, 24);

    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut performer = GridPerformer::new(&mut grid);
                for byte in &buf[..n] {
                    parser.advance(&mut performer, *byte);
                }
            }
        }
    }
    print_grid(&grid);
}

#[cfg(test)]
mod tests {
    use gunter_core::grid::Grid;
    use gunter_term::performer::GridPerformer;

    fn feed(grid: &mut Grid, input: &[u8]) {
        let mut parser = vte::Parser::new();
        let mut performer = GridPerformer::new(grid);
        for &byte in input {
            parser.advance(&mut performer, byte);
        }
    }

    #[test]
    fn plain_text_appears_in_grid() {
        let mut grid = Grid::new(80, 24);
        feed(&mut grid, b"hello");
        assert_eq!(grid.cells[0].ch, 'h');
        assert_eq!(grid.cells[1].ch, 'e');
        assert_eq!(grid.cells[2].ch, 'l');
        assert_eq!(grid.cells[3].ch, 'l');
        assert_eq!(grid.cells[4].ch, 'o');
    }

    #[test]
    fn ansi_colors_applied_in_grid() {
        let mut grid = Grid::new(80, 24);
        // SGR 31 = red fg, then 'R'
        feed(&mut grid, b"\x1b[31mR");
        use gunter_core::grid::Color;
        assert_eq!(grid.cells[0].fg, Color { r: 224, g: 108, b: 117 }); // Atom One Dark red
    }

    #[test]
    fn cursor_position_after_text() {
        let mut grid = Grid::new(80, 24);
        feed(&mut grid, b"hi");
        assert_eq!(grid.cursor, (2, 0));
    }
}
