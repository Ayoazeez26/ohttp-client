//! The OHTTP relay: forwards encapsulated `POST`s to the gateway and tunnels
//! `CONNECT`s, the key-fetch bootstrap, to the authority they name.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use super::server::{read_request, write_response, HttpRequest};

/// Relay accept loop. Each connection gets its own thread because a
/// `CONNECT` tunnel stays open for as long as the client uses it.
pub(super) fn serve_relay(
    listener: TcpListener,
    gateway_url: String,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let Ok(mut stream) = stream else { continue };
            let gateway_url = gateway_url.clone();
            std::thread::spawn(move || {
                let Some(req) = read_request(&mut stream) else {
                    return;
                };
                match req.method.as_str() {
                    "POST" => forward_to_gateway(&mut stream, &gateway_url, req),
                    "CONNECT" => {
                        let _ = tunnel(stream, &req.path);
                    }
                    _ => write_response(&mut stream, 405, &[], b"Method Not Allowed"),
                }
            });
        }
    })
}

/// Forward the encapsulated request to the gateway with only its content
/// type (RFC 9458 Section 5), and pass back the gateway's status, content
/// type and content.
fn forward_to_gateway(client: &mut TcpStream, gateway_url: &str, req: HttpRequest) {
    let mut forward = bitreq::post(gateway_url);
    if let Some(content_type) = req.header("content-type") {
        forward = forward.with_header("content-type", content_type);
    }
    let Ok(res) = forward.with_body(req.body).send() else {
        return write_response(client, 502, &[], b"Bad Gateway");
    };
    let headers: Vec<_> = res
        .headers
        .get("content-type")
        .map(|v| ("content-type".to_string(), v.clone()))
        .into_iter()
        .collect();
    write_response(client, res.status_code as u16, &headers, res.as_bytes());
}

/// Dial `authority`, reply `200`, then pipe bytes both ways until either
/// side closes.
fn tunnel(mut client: TcpStream, authority: &str) -> Option<()> {
    let mut upstream = TcpStream::connect(authority).ok()?;
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
