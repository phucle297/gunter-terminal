use std::sync::Arc;
use std::sync::mpsc;

use gunter_core::grid::Grid;
use gunter_renderer::GunterRenderer;
use gunter_term::performer::GridPerformer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const COLS: u16 = 80;
const ROWS: u16 = 24;

struct GunterApp {
    window: Option<Arc<Window>>,
    renderer: Option<GunterRenderer>,
    grid: Grid,
    pty_rx: mpsc::Receiver<Vec<u8>>,
    pty_tx: mpsc::SyncSender<Vec<u8>>,
    pty_resize_tx: mpsc::SyncSender<(u16, u16)>,
    parser: vte::Parser,
    modifiers: winit::event::Modifiers,
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

            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let Some(bytes) = translate_key(&event) {
                        let _ = self.pty_tx.try_send(bytes);
                    }
                }
            }

            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                    let (cw, ch) = r.cell_size();
                    if cw > 0.0 && ch > 0.0 {
                        let cols = (size.width as f32 / cw) as u16;
                        let rows = (size.height as f32 / ch) as u16;
                        if cols > 0 && rows > 0 {
                            let _ = self.pty_resize_tx.try_send((cols, rows));
                            self.grid.resize(cols, rows);
                        }
                    }
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

fn translate_key(event: &winit::event::KeyEvent) -> Option<Vec<u8>> {
    match &event.logical_key {
        Key::Character(s) => Some(s.as_str().as_bytes().to_vec()),
        Key::Named(named) => match named {
            NamedKey::Enter => Some(b"\r".to_vec()),
            NamedKey::Backspace => Some(b"\x7f".to_vec()),
            NamedKey::Tab => Some(b"\x09".to_vec()),
            NamedKey::Escape => Some(b"\x1b".to_vec()),
            NamedKey::ArrowUp => Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => Some(b"\x1b[D".to_vec()),
            NamedKey::Home => Some(b"\x1b[H".to_vec()),
            NamedKey::End => Some(b"\x1b[F".to_vec()),
            NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
            NamedKey::Delete => Some(b"\x1b[3~".to_vec()),
            NamedKey::Insert => Some(b"\x1b[2~".to_vec()),
            NamedKey::F1 => Some(b"\x1bOP".to_vec()),
            NamedKey::F2 => Some(b"\x1bOQ".to_vec()),
            NamedKey::F3 => Some(b"\x1bOR".to_vec()),
            NamedKey::F4 => Some(b"\x1bOS".to_vec()),
            NamedKey::F5 => Some(b"\x1b[15~".to_vec()),
            NamedKey::F6 => Some(b"\x1b[17~".to_vec()),
            NamedKey::F7 => Some(b"\x1b[18~".to_vec()),
            NamedKey::F8 => Some(b"\x1b[19~".to_vec()),
            NamedKey::F9 => Some(b"\x1b[20~".to_vec()),
            NamedKey::F10 => Some(b"\x1b[21~".to_vec()),
            NamedKey::F11 => Some(b"\x1b[23~".to_vec()),
            NamedKey::F12 => Some(b"\x1b[24~".to_vec()),
            _ => None,
        },
        _ => None,
    }
}

fn main() {
    env_logger::init();

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let (key_tx, key_rx) = mpsc::sync_channel::<Vec<u8>>(64);
    let (resize_tx, resize_rx) = mpsc::sync_channel::<(u16, u16)>(4);

    std::thread::spawn(move || {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use std::io::{Read, Write};

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 })
            .expect("openpty failed");

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let cmd = CommandBuilder::new(&shell);
        let _child = pair.slave.spawn_command(cmd).expect("spawn shell");
        drop(pair.slave);

        let mut writer = pair.master.take_writer().expect("take writer");
        let mut reader = pair.master.try_clone_reader().expect("clone reader");

        std::thread::spawn(move || {
            while let Ok(bytes) = key_rx.recv() {
                let _ = writer.write_all(&bytes);
            }
        });

        let mut buf = [0u8; 4096];
        loop {
            while let Ok((cols, rows)) = resize_rx.try_recv() {
                let _ = pair.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            }
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
        pty_tx: key_tx,
        pty_resize_tx: resize_tx,
        parser: vte::Parser::new(),
        modifiers: winit::event::Modifiers::default(),
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
