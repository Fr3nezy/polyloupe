//! "Open files in the same window".
//!
//! While the preference is on, the running window listens on a loopback port and records it, with
//! a random token, in `%LOCALAPPDATA%\PolyLoupe\instance`. A later launch with a file (a double
//! click in Explorer) sends the path there and exits; if nothing answers it opens normally.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui;

fn record_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("PolyLoupe").join("instance"))
}

/// Hands `file` to a running window. True when it accepted it.
pub fn forward(file: &Path) -> bool {
    let Some(text) = record_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return false };
    let Some((port, token)) = text.trim().split_once(' ') else { return false };
    let Ok(port) = port.parse::<u16>() else { return false };
    let file = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if writeln!(stream, "{token}\n{}", file.display()).is_err() {
        return false;
    }
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).is_ok() && reply.trim() == "ok"
}

/// Accepts files from later launches until dropped.
pub struct Server {
    pub files: Receiver<PathBuf>,
    record: PathBuf,
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
}

pub fn listen(ctx: &egui::Context) -> Option<Server> {
    let record = record_path()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = new_token();
    std::fs::create_dir_all(record.parent()?).ok()?;
    std::fs::write(&record, format!("{port} {token}")).ok()?;

    let (tx, files) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let (thread_stop, thread_token, ctx) = (stop.clone(), token.clone(), ctx.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if thread_stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut lines = BufReader::new(&stream).lines();
            let (Some(Ok(token)), Some(Ok(path))) = (lines.next(), lines.next()) else { continue };
            if token != thread_token {
                continue;
            }
            let _ = (&stream).write_all(b"ok\n");
            let _ = tx.send(PathBuf::from(path));
            ctx.request_repaint();
        }
    });
    Some(Server { files, record, port, token, stop })
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the blocked accept so the thread sees the stop flag.
        let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), Duration::from_millis(100));
        // Another window may have taken over the record since; only remove ours.
        if std::fs::read_to_string(&self.record).is_ok_and(|t| t.ends_with(&self.token)) {
            let _ = std::fs::remove_file(&self.record);
        }
    }
}

fn new_token() -> String {
    use std::hash::BuildHasher;
    let seed = (std::process::id(), std::time::SystemTime::now());
    format!("{:016x}", std::collections::hash_map::RandomState::new().hash_one(seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_a_file_to_the_listening_window() {
        let ctx = egui::Context::default();
        let server = listen(&ctx).expect("listen");
        let file = std::env::temp_dir().join("model.glb");
        assert!(forward(&file));
        let got = server.files.recv_timeout(Duration::from_secs(2)).expect("received");
        assert_eq!(got, file);
        drop(server);
        // Nothing listens any more: the launch opens its own window.
        assert!(!forward(&file));
    }
}
