use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;

use gunter_core::config::Config;
use gunter_core::grid::Grid;
use gunter_core::layout::{Axis, Layout, Rect};
use gunter_renderer::GunterRenderer;
use gunter_term::performer::GridPerformer;
use uuid::Uuid;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const COLS: u16 = 80;
const ROWS: u16 = 24;

struct PtyHandle {
    pty_tx: mpsc::SyncSender<Vec<u8>>,
    pty_resize_tx: mpsc::SyncSender<(u16, u16)>,
    pty_rx: mpsc::Receiver<Vec<u8>>,
}

struct SessionState {
    grid: Grid,
    parser: vte::Parser,
    pty: PtyHandle,
}

impl SessionState {
    fn new(cols: u16, rows: u16) -> Self {
        Self::new_with_shell(cols, rows, None, &[])
    }

    fn new_with_shell(cols: u16, rows: u16, shell_program: Option<&str>, shell_args: &[String]) -> Self {
        let (pty_out_tx, pty_out_rx) = mpsc::channel::<Vec<u8>>();
        let (key_tx, key_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let (resize_tx, resize_rx) = mpsc::sync_channel::<(u16, u16)>(4);

        let shell = shell_program
            .map(|s| s.to_string())
            .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()));
        let args: Vec<String> = shell_args.to_vec();

        std::thread::spawn(move || {
            use portable_pty::{native_pty_system, CommandBuilder, PtySize};
            use std::io::{Read, Write};

            let pty_system = native_pty_system();
            let pair = pty_system
                .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
                .expect("openpty failed");

            let mut cmd = CommandBuilder::new(&shell);
            for arg in &args { cmd.arg(arg); }
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
                while let Ok((c, r)) = resize_rx.try_recv() {
                    let _ = pair.master.resize(PtySize {
                        rows: r,
                        cols: c,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if pty_out_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        SessionState {
            grid: Grid::new(cols, rows),
            parser: vte::Parser::new(),
            pty: PtyHandle {
                pty_tx: key_tx,
                pty_resize_tx: resize_tx,
                pty_rx: pty_out_rx,
            },
        }
    }
}

struct GunterApp {
    window: Option<Arc<Window>>,
    renderer: Option<GunterRenderer>,
    sessions: HashMap<Uuid, SessionState>,
    layout: Layout,
    active_id: Uuid,
    tab_layouts: Vec<(Layout, HashMap<Uuid, SessionState>)>,
    active_tab: usize,
    modifiers: winit::event::Modifiers,
    selection_start: Option<(u16, u16)>,
    selection_end: Option<(u16, u16)>,
    cursor_pos_px: (f64, f64),
    mouse_pressed: bool,
    config: Config,
    config_rx: mpsc::Receiver<()>,
}

impl GunterApp {
    fn extract_selection_text(&self, start: (u16, u16), end: (u16, u16)) -> String {
        let session = match self.sessions.get(&self.active_id) {
            Some(s) => s,
            None => return String::new(),
        };
        let (mut r1, mut c1) = (start.1 as usize, start.0 as usize);
        let (mut r2, mut c2) = (end.1 as usize, end.0 as usize);
        if (r1, c1) > (r2, c2) {
            std::mem::swap(&mut r1, &mut r2);
            std::mem::swap(&mut c1, &mut c2);
        }
        let cols = session.grid.cols as usize;
        let mut out = String::new();
        for row in r1..=r2 {
            let start_col = if row == r1 { c1 } else { 0 };
            let end_col = if row == r2 { c2 } else { cols - 1 };
            for col in start_col..=end_col {
                let idx = row * cols + col;
                if idx < session.grid.cells.len() {
                    out.push(session.grid.cells[idx].ch);
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
                            // Ctrl+Shift+C — copy selection
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
                            // Ctrl+Shift+V — paste
                            Key::Character(s) if s.as_str().eq_ignore_ascii_case("v") => {
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    if let Ok(text) = clipboard.get_text() {
                                        if let Some(session) = self.sessions.get(&self.active_id) {
                                            let data = if session.grid.bracketed_paste {
                                                let mut v = b"\x1b[200~".to_vec();
                                                v.extend_from_slice(text.as_bytes());
                                                v.extend_from_slice(b"\x1b[201~");
                                                v
                                            } else {
                                                text.into_bytes()
                                            };
                                            let _ = session.pty.pty_tx.try_send(data);
                                        }
                                    }
                                }
                                return;
                            }
                            // Ctrl+Shift+H — split horizontal (side by side)
                            Key::Character(s) if s.as_str().eq_ignore_ascii_case("h") => {
                                let new_id = Uuid::new_v4();
                                let new_session = SessionState::new(COLS / 2, ROWS);
                                self.sessions.insert(new_id, new_session);
                                self.layout = self.layout.clone().split_leaf(
                                    self.active_id,
                                    Axis::Horizontal,
                                    new_id,
                                );
                                self.active_id = new_id;
                                return;
                            }
                            // Ctrl+Shift+V (vertical split) — already handled above as paste
                            // Use Ctrl+Shift+E for vertical split to avoid conflict
                            Key::Character(s) if s.as_str().eq_ignore_ascii_case("e") => {
                                let new_id = Uuid::new_v4();
                                let new_session = SessionState::new(COLS, ROWS / 2);
                                self.sessions.insert(new_id, new_session);
                                self.layout = self.layout.clone().split_leaf(
                                    self.active_id,
                                    Axis::Vertical,
                                    new_id,
                                );
                                self.active_id = new_id;
                                return;
                            }
                            // Ctrl+Shift+T — new tab
                            Key::Character(s) if s.as_str().eq_ignore_ascii_case("t") => {
                                let new_id = Uuid::new_v4();
                                let new_session = SessionState::new(COLS, ROWS);
                                let old_layout =
                                    std::mem::replace(&mut self.layout, Layout::leaf(new_id));
                                let old_sessions = std::mem::take(&mut self.sessions);
                                self.tab_layouts.push((old_layout, old_sessions));
                                self.sessions.insert(new_id, new_session);
                                self.active_id = new_id;
                                self.active_tab = self.tab_layouts.len();
                                return;
                            }
                            // Ctrl+Shift+W — close active tab/pane
                            Key::Character(s) if s.as_str().eq_ignore_ascii_case("w") => {
                                let target = self.active_id;
                                self.sessions.remove(&target);
                                match self.layout.clone().remove_leaf(target) {
                                    Some(new_layout) => {
                                        self.layout = new_layout;
                                        self.active_id = self.layout.focused();
                                    }
                                    None => {
                                        // last pane in tab — pop previous tab or exit
                                        if let Some((prev_layout, prev_sessions)) =
                                            self.tab_layouts.pop()
                                        {
                                            self.layout = prev_layout;
                                            self.sessions = prev_sessions;
                                            self.active_id = self.layout.focused();
                                            self.active_tab = self.tab_layouts.len();
                                        } else {
                                            event_loop.exit();
                                        }
                                    }
                                }
                                return;
                            }
                            // Ctrl+Shift+Tab — cycle to previous tab
                            Key::Named(NamedKey::Tab) => {
                                if !self.tab_layouts.is_empty() {
                                    let cur_layout =
                                        std::mem::replace(&mut self.layout, Layout::leaf(Uuid::nil()));
                                    let cur_sessions = std::mem::take(&mut self.sessions);
                                    self.tab_layouts.push((cur_layout, cur_sessions));
                                    let (prev_layout, prev_sessions) =
                                        self.tab_layouts.remove(0);
                                    self.layout = prev_layout;
                                    self.sessions = prev_sessions;
                                    self.active_id = self.layout.focused();
                                    self.active_tab = self.active_tab.saturating_sub(1);
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                    // Ctrl+Tab (no shift) — cycle to next pane within layout
                    if ctrl && !shift {
                        if let Key::Named(NamedKey::Tab) = &event.logical_key {
                            let rects = self.layout.rects(Rect::default());
                            let ids: Vec<Uuid> = rects.iter().map(|(id, _)| *id).collect();
                            if let Some(pos) = ids.iter().position(|&id| id == self.active_id) {
                                self.active_id = ids[(pos + 1) % ids.len()];
                            }
                            return;
                        }
                        // Ctrl+arrows — send modifier sequences
                        let ctrl_arrow: Option<&[u8]> = match &event.logical_key {
                            Key::Named(NamedKey::ArrowUp)    => Some(b"\x1b[1;5A"),
                            Key::Named(NamedKey::ArrowDown)  => Some(b"\x1b[1;5B"),
                            Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[1;5C"),
                            Key::Named(NamedKey::ArrowLeft)  => Some(b"\x1b[1;5D"),
                            _ => None,
                        };
                        if let Some(seq) = ctrl_arrow {
                            if let Some(session) = self.sessions.get(&self.active_id) {
                                let _ = session.pty.pty_tx.try_send(seq.to_vec());
                            }
                            return;
                        }
                        // Ctrl+letter — translate to control character (fallback for platforms
                        // where winit does not apply Ctrl in logical_key)
                        if let Key::Character(s) = &event.logical_key {
                            let ch = s.chars().next().unwrap_or('\0').to_ascii_lowercase();
                            if ch >= 'a' && ch <= 'z' {
                                let byte = (ch as u8) - b'a' + 1;
                                if let Some(session) = self.sessions.get(&self.active_id) {
                                    let _ = session.pty.pty_tx.try_send(vec![byte]);
                                }
                                return;
                            }
                        }
                    }
                    if let Some(bytes) = translate_key(&event) {
                        if let Some(session) = self.sessions.get(&self.active_id) {
                            let _ = session.pty.pty_tx.try_send(bytes);
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(session) = self.sessions.get_mut(&self.active_id) {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
                        winit::event::MouseScrollDelta::PixelDelta(p) => (p.y / 20.0) as i32,
                    };
                    let max_offset = session.grid.scrollback.len();
                    let current = session.grid.scroll_offset as i32;
                    let new_offset = (current - lines).clamp(0, max_offset as i32) as usize;
                    if new_offset != session.grid.scroll_offset {
                        session.grid.scroll_offset = new_offset;
                        session.grid.mark_all_dirty();
                    }
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
                if let Some(session) = self.sessions.get(&self.active_id) {
                    if session.grid.mouse_reporting {
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
                            let seq = if session.grid.mouse_sgr {
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
                            let _ = session.pty.pty_tx.try_send(seq.into_bytes());
                        }
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos_px = (position.x, position.y);
                if self.mouse_pressed {
                    if let Some(r) = &self.renderer {
                        if let Some(session) = self.sessions.get(&self.active_id) {
                            let (cw, ch) = r.cell_size();
                            let cx = (position.x / cw as f64) as u16;
                            let cy = (position.y / ch as f64) as u16;
                            self.selection_end = Some((
                                cx.min(session.grid.cols - 1),
                                cy.min(session.grid.rows - 1),
                            ));
                        }
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
                            if let Some(session) = self.sessions.get_mut(&self.active_id) {
                                let _ = session.pty.pty_resize_tx.try_send((cols, rows));
                                session.grid.resize(cols, rows);
                            }
                        }
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                for (_, session) in &mut self.sessions {
                    while let Ok(bytes) = session.pty.pty_rx.try_recv() {
                        let mut perf = GridPerformer::new(&mut session.grid);
                        for &b in &bytes {
                            session.parser.advance(&mut perf, b);
                        }
                    }
                }
                if let Some(r) = &mut self.renderer {
                    if let Some(session) = self.sessions.get_mut(&self.active_id) {
                        r.render(&mut session.grid);
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Hot reload: apply config changes if watcher fired
        if self.config_rx.try_recv().is_ok() {
            self.config.reload();
            // Mark all grids dirty so renderer picks up any theme/font changes
            for (_, session) in &mut self.sessions {
                session.grid.mark_all_dirty();
            }
        }
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

fn start_config_watcher(tx: mpsc::SyncSender<()>) {
    use notify::{EventKind, RecursiveMode, Watcher};
    let Some(path) = gunter_core::config::config_path() else { return };
    if !path.exists() { return; }
    std::thread::spawn(move || {
        let (watch_tx, watch_rx) = mpsc::channel();
        let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                match ev.kind {
                    EventKind::Modify(_) | EventKind::Create(_) => { let _ = watch_tx.send(()); }
                    _ => {}
                }
            }
        }) {
            Ok(w) => w,
            Err(_) => return,
        };
        if watcher.watch(&path, RecursiveMode::NonRecursive).is_err() { return; }
        loop {
            if watch_rx.recv().is_err() { break; }
            let _ = tx.try_send(());
        }
    });
}

fn main() {
    env_logger::init();

    let config = Config::load();
    let (cfg_tx, cfg_rx) = mpsc::sync_channel::<()>(4);
    start_config_watcher(cfg_tx);

    let initial_id = Uuid::new_v4();
    let initial_session = SessionState::new_with_shell(
        COLS, ROWS,
        Some(&config.shell.program),
        &config.shell.args,
    );
    let mut sessions = HashMap::new();
    sessions.insert(initial_id, initial_session);

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = GunterApp {
        window: None,
        renderer: None,
        sessions,
        layout: Layout::leaf(initial_id),
        active_id: initial_id,
        tab_layouts: Vec::new(),
        active_tab: 0,
        modifiers: winit::event::Modifiers::default(),
        selection_start: None,
        selection_end: None,
        cursor_pos_px: (0.0, 0.0),
        mouse_pressed: false,
        config,
        config_rx: cfg_rx,
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
