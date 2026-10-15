//! Minimal hand-rolled HTTP/1.1 server shared by the target and gateway.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

pub(super) struct HttpRequest {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
}

impl HttpRequest {
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Minimal HTTP/1.1 server loop: one connection at a time, one request per
/// connection.
pub(super) fn serve(
    listener: TcpListener,
    shutdown: Arc<AtomicBool>,
    handler: impl Fn(HttpRequest) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + 'static,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let Ok(mut stream) = stream else { continue };
            if let Some(req) = read_request(&mut stream) {
                let (status, headers, body) = handler(req);
                write_response(&mut stream, status, &headers, &body);
            }
        }
    })
}

fn read_request(stream: &mut TcpStream) -> Option<HttpRequest> {
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }

    let req = HttpRequest {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let content_length: usize = req
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;

    Some(HttpRequest { body, ..req })
}

fn write_response(stream: &mut TcpStream, status: u16, headers: &[(String, String)], body: &[u8]) {
    // `connection: close` keeps the relay's pooled hyper client from reusing
    // a connection this one-request-per-connection server has already closed.
    let mut response = format!(
        "HTTP/1.1 {status} OK\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
