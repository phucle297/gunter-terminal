use std::sync::Arc;
use std::sync::mpsc;

use gunter_core::grid::Grid;
use gunter_renderer::GunterRenderer;
use gunter_term::performer::GridPerformer;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const COLS: u16 = 80;
const ROWS: u16 = 24;

struct GunterApp {
    window: Option<Arc<Window>>,
    renderer: Option<GunterRenderer>,
    grid: Grid,
    pty_rx: mpsc::Receiver<Vec<u8>>,
    parser: vte::Parser,
}

impl ApplicationHandler for GunterApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("Gunter")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                COLS as u32 * 8,
                ROWS as u32 * 16,
            ));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let renderer = pollster::block_on(GunterRenderer::new(window.clone(), COLS, ROWS));

        // Resize window to exact cell grid dimensions once cell size is known
        let (cw, ch) = renderer.cell_size();
        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(
            (COLS as f32 * cw) as u32,
            (ROWS as f32 * ch) as u32,
        ));

        self.window = Some(window);
        self.renderer = Some(renderer);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }

            WindowEvent::RedrawRequested => {
                while let Ok(bytes) = self.pty_rx.try_recv() {
                    let mut perf = GridPerformer::new(&mut self.grid);
                    for &b in &bytes {
                        self.parser.advance(&mut perf, b);
                    }
                }
                if let Some(r) = &mut self.renderer {
                    r.render(&self.grid);
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

fn main() {
    env_logger::init();

    let (tx, rx) = mpsc::channel::<Vec<u8>>();

    std::thread::spawn(move || {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use std::io::Read;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 })
            .expect("openpty failed");

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let cmd = CommandBuilder::new(&shell);
        let _child = pair.slave.spawn_command(cmd).expect("spawn shell");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = GunterApp {
        window: None,
        renderer: None,
        grid: Grid::new(COLS, ROWS),
        pty_rx: rx,
        parser: vte::Parser::new(),
    };
    event_loop.run_app(&mut app).expect("run_app");
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
        assert_eq!(grid.cells[4].ch, 'o');
    }

    #[test]
    fn ansi_colors_applied_in_grid() {
        let mut grid = Grid::new(80, 24);
        feed(&mut grid, b"\x1b[31mR");
        use gunter_core::grid::Color;
        assert_eq!(grid.cells[0].fg, Color { r: 224, g: 108, b: 117 });
    }
}
