//! Local-only HTTP fixture: a TcpListener on 127.0.0.1:0 serving canned responses in order.
//! No real external services are contacted.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Fixture {
    pub base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// Serve one canned response per accepted connection, in order.
/// An empty response means "accept, drain the request, then stall forever".
pub fn serve(responses: Vec<Vec<u8>>) -> Fixture {
    serve_delayed(
        responses
            .into_iter()
            .map(|bytes| (std::time::Duration::ZERO, bytes))
            .collect(),
    )
}

pub fn serve_delayed(responses: Vec<(std::time::Duration, Vec<u8>)>) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = requests.clone();
    thread::spawn(move || {
        let mut held = Vec::new();
        for (delay, resp) in responses {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(_) => return,
            };
            let request = drain_request(&mut stream);
            sink.lock().unwrap().push(request);
            thread::sleep(delay);
            if resp.is_empty() {
                held.push(stream);
                thread::sleep(std::time::Duration::from_secs(60));
                return;
            }
            let _ = stream.write_all(&resp);
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });
    Fixture {
        base_url: format!("http://{addr}"),
        requests,
    }
}

pub fn image_response(bytes: &[u8]) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    )
    .into_bytes()
    .into_iter()
    .chain(bytes.iter().copied())
    .collect()
}

pub fn redirect(location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 301 Moved Permanently\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

pub fn tiny_png() -> Vec<u8> {
    let img = image::ImageBuffer::from_fn(2, 2, |_, _| image::Rgba([9, 200, 40, 255]));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

fn drain_request(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

pub fn header_map(request: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in request.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            map.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    map
}
