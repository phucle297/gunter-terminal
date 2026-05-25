use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub fn socket_path(id: Uuid) -> PathBuf {
    std::env::temp_dir().join(format!("gunter-{}.sock", id))
}

/// Broadcast PTY output to multiple subscribers (app + socket clients).
#[derive(Clone, Default)]
pub struct OutputBroadcast {
    subs: Arc<Mutex<Vec<mpsc::SyncSender<Vec<u8>>>>>,
}

impl OutputBroadcast {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::sync_channel(512);
        self.subs.lock().unwrap().push(tx);
        rx
    }

    pub fn publish(&self, data: Vec<u8>) {
        let mut subs = self.subs.lock().unwrap();
        subs.retain(|tx| tx.try_send(data.clone()).is_ok());
    }
}

/// Start a Unix socket server for `id`. PTY output comes via `broadcast`;
/// bytes from clients are forwarded to `pty_in`.
pub fn start_socket_server(
    id: Uuid,
    pty_in: mpsc::SyncSender<Vec<u8>>,
    broadcast: OutputBroadcast,
) {
    let path = socket_path(id);
    let _ = std::fs::remove_file(&path);

    std::thread::spawn(move || {
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixListener;

            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("gunter: socket bind {}: {e}", path.display());
                    return;
                }
            };

            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let out_rx = broadcast.subscribe();
                let pty_in = pty_in.clone();
                let mut write_half = match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                // PTY output → client
                std::thread::spawn(move || {
                    while let Ok(bytes) = out_rx.recv() {
                        if write_half.write_all(&bytes).is_err() {
                            break;
                        }
                    }
                });
                // client input → PTY
                let mut read_half = stream;
                let mut buf = [0u8; 4096];
                loop {
                    match read_half.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if pty_in.try_send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        #[cfg(not(unix))]
        {
            eprintln!("gunter: socket server not yet supported on this platform");
        }
    });
}
