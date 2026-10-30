//! Local focus handoff keeps one library window while readers stay independent.
use eframe::egui;
use std::{
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    time::Duration,
};
pub fn focus_existing(data: &Path) -> bool {
    let Some(address) = std::fs::read_to_string(data.join("library.port"))
        .ok()
        .and_then(|s| s.parse::<SocketAddr>().ok())
        .filter(|a| a.ip().is_loopback())
    else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(150)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
    if stream.write_all(b"focus\n").is_err() {
        return false;
    }
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).is_ok() && answer.trim() == "yomi-library-v1"
}
pub fn listen(data: &Path, ctx: egui::Context) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        return;
    };
    let Ok(address) = listener.local_addr() else {
        return;
    };
    if std::fs::write(data.join("library.port"), address.to_string()).is_err() {
        return;
    }
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
            let mut request = String::new();
            if BufReader::new(&mut stream).read_line(&mut request).is_ok()
                && request.trim() == "focus"
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint();
                let _ = stream.write_all(b"yomi-library-v1\n");
            }
        }
    });
}
