//! Generic HTTP `CONNECT` proxy standing in for a relay's key-fetch tunnel.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// A minimal HTTP `CONNECT` proxy: read the request line and headers, dial
/// the requested `host:port`, reply `200`, then pipe bytes bidirectionally.
pub(super) fn serve_connect_proxy(
    listener: TcpListener,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let Ok(client) = stream else { continue };
            std::thread::spawn(move || {
                let _ = handle_connect(client);
            });
        }
    })
}

fn handle_connect(client: TcpStream) -> Option<()> {
    let mut reader = BufReader::new(client.try_clone().ok()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    if parts.next()? != "CONNECT" {
        return None;
    }
    let target = parts.next()?.to_string();

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line.trim_end().is_empty() {
            break;
        }
    }

    let mut upstream = TcpStream::connect(&target).ok()?;
    let mut client = reader.into_inner();
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .ok()?;

    let mut upstream_read = upstream.try_clone().ok()?;
    let mut client_read = client.try_clone().ok()?;
    let relay_up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut client_read, &mut upstream);
    });
    let _ = std::io::copy(&mut upstream_read, &mut client);
    let _ = relay_up.join();
    Some(())
}
