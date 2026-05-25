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
    selection_start: Option<(u16, u16)>,
    selection_end: Option<(u16, u16)>,
    cursor_pos_px: (f64, f64),
    mouse_pressed: bool,
}

impl GunterApp {
    fn extract_selection_text(&self, start: (u16, u16), end: (u16, u16)) -> String {
        let (mut r1, mut c1) = (start.1 as usize, start.0 as usize);
        let (mut r2, mut c2) = (end.1 as usize, end.0 as usize);
        if (r1, c1) > (r2, c2) {
            std::mem::swap(&mut r1, &mut r2);
            std::mem::swap(&mut c1, &mut c2);
        }
        let cols = self.grid.cols as usize;
        let mut out = String::new();
        for row in r1..=r2 {
            let start_col = if row == r1 { c1 } else { 0 };
            let end_col = if row == r2 { c2 } else { cols - 1 };
            for col in start_col..=end_col {
                let idx = row * cols + col;
                if idx < self.grid.cells.len() {
                    out.push(self.grid.cells[idx].ch);
                }
            }
            if row < r2 {
                out.push('\n');
            }
        }
        out.trim_end().to_string()
    }
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
                    let ctrl = self.modifiers.state().control_key();
                    let shift = self.modifiers.state().shift_key();
                    if ctrl && shift {
                        match &event.logical_key {
                            Key::Character(s) if s.as_str().eq_ignore_ascii_case("c") => {
                                if let (Some(start), Some(end)) =
                                    (self.selection_start, self.selection_end)
                                {
                                    let text = self.extract_selection_text(start, end);
                                    if !text.is_empty() {
                                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                            let _ = clipboard.set_text(&text);
                                        }
                                    }
                                }
                                return;
                            }
                            Key::Character(s) if s.as_str().eq_ignore_ascii_case("v") => {
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    if let Ok(text) = clipboard.get_text() {
                                        let data = if self.grid.bracketed_paste {
                                            let mut v = b"\x1b[200~".to_vec();
                                            v.extend_from_slice(text.as_bytes());
                                            v.extend_from_slice(b"\x1b[201~");
                                            v
                                        } else {
                                            text.into_bytes()
                                        };
                                        let _ = self.pty_tx.try_send(data);
                                    }
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                    if let Some(bytes) = translate_key(&event) {
                        let _ = self.pty_tx.try_send(bytes);
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
                    winit::event::MouseScrollDelta::PixelDelta(p) => (p.y / 20.0) as i32,
                };
                let max_offset = self.grid.scrollback.len();
                let current = self.grid.scroll_offset as i32;
                let new_offset =
                    (current - lines).clamp(0, max_offset as i32) as usize;
                if new_offset != self.grid.scroll_offset {
                    self.grid.scroll_offset = new_offset;
                    self.grid.mark_all_dirty();
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                use winit::event::{ElementState as ES, MouseButton};
                match (button, state) {
                    (MouseButton::Left, ES::Pressed) => {
                        self.mouse_pressed = true;
                        if let Some(r) = &self.renderer {
                            let (cw, ch) = r.cell_size();
                            let cx = (self.cursor_pos_px.0 / cw as f64) as u16;
                            let cy = (self.cursor_pos_px.1 / ch as f64) as u16;
                            self.selection_start = Some((cx, cy));
                            self.selection_end = None;
                        }
                    }
                    (MouseButton::Left, ES::Released) => {
                        self.mouse_pressed = false;
                    }
                    _ => {}
                }
                if self.grid.mouse_reporting {
                    if let Some(r) = &self.renderer {
                        let (cw, ch) = r.cell_size();
                        let cx = (self.cursor_pos_px.0 / cw as f64) as u16 + 1;
                        let cy = (self.cursor_pos_px.1 / ch as f64) as u16 + 1;
                        let btn = match button {
                            winit::event::MouseButton::Left => 0u8,
                            winit::event::MouseButton::Middle => 1,
                            winit::event::MouseButton::Right => 2,
                            _ => 3,
                        };
                        let press = state == ES::Pressed;
                        let seq = if self.grid.mouse_sgr {
                            format!(
                                "\x1b[<{};{};{}{}",
                                btn,
                                cx,
                                cy,
                                if press { 'M' } else { 'm' }
                            )
                        } else {
                            let cb = btn + 32 + if press { 0 } else { 3 };
                            format!(
                                "\x1b[M{}{}{}",
                                cb as char,
                                (cx + 32) as u8 as char,
                                (cy + 32) as u8 as char
                            )
                        };
                        let _ = self.pty_tx.try_send(seq.into_bytes());
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos_px = (position.x, position.y);
                if self.mouse_pressed {
                    if let Some(r) = &self.renderer {
                        let (cw, ch) = r.cell_size();
                        let cx = (position.x / cw as f64) as u16;
                        let cy = (position.y / ch as f64) as u16;
                        self.selection_end = Some((
                            cx.min(self.grid.cols - 1),
                            cy.min(self.grid.rows - 1),
                        ));
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
                    r.render(&mut self.grid);
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
        selection_start: None,
        selection_end: None,
        cursor_pos_px: (0.0, 0.0),
        mouse_pressed: false,
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
