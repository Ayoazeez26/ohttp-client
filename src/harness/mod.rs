//! Local target, OHTTP gateway and OHTTP relay for end-to-end tests. Each is a
//! hand-rolled HTTP/1.1 server on its own thread, stopped when the
//! [`TestHarness`] drops.

mod gateway;
mod relay;
mod server;

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use gateway::handle_gateway_request;
use relay::serve_relay;
use server::serve;

const GATEWAY_PATH: &str = "/.well-known/ohttp-gateway";

pub struct TestHarness {
    relay_url: String,
    gateway_url: String,
    target_url: String,
    shutdown: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    server_addrs: Vec<SocketAddr>,
}

impl TestHarness {
    pub fn start() -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut server_addrs = Vec::new();

        // Target: echoes the method, path and body.
        let target_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target_addr = target_listener.local_addr().unwrap();
        server_addrs.push(target_addr);
        threads.push(serve(target_listener, shutdown.clone(), |req| {
            let mut body = format!("{} {} ", req.method, req.path).into_bytes();
            body.extend_from_slice(&req.body);
            (
                200,
                vec![("content-type".into(), "text/plain".into())],
                body,
            )
        }));

        // Gateway.
        let key_config = ohttp::KeyConfig::new(
            1,
            ohttp::hpke::Kem::X25519Sha256,
            vec![ohttp::SymmetricSuite::new(
                ohttp::hpke::Kdf::HkdfSha256,
                ohttp::hpke::Aead::ChaCha20Poly1305,
            )],
        )
        .unwrap();
        let ohttp_server = Arc::new(ohttp::Server::new(key_config).unwrap());
        let gateway_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let gateway_addr = gateway_listener.local_addr().unwrap();
        server_addrs.push(gateway_addr);
        threads.push(serve(gateway_listener, shutdown.clone(), move |req| {
            handle_gateway_request(&ohttp_server, req)
        }));

        // Relay.
        let gateway_url = format!("http://127.0.0.1:{}{}", gateway_addr.port(), GATEWAY_PATH);
        let relay_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let relay_addr = relay_listener.local_addr().unwrap();
        server_addrs.push(relay_addr);
        threads.push(serve_relay(
            relay_listener,
            gateway_url.clone(),
            shutdown.clone(),
        ));

        TestHarness {
            relay_url: format!("http://127.0.0.1:{}/", relay_addr.port()),
            gateway_url,
            target_url: format!("http://127.0.0.1:{}", target_addr.port()),
            shutdown,
            threads,
            server_addrs,
        }
    }

    /// POST encapsulated requests here, or use it as a `CONNECT` proxy for
    /// the key fetch.
    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }

    /// Gateway key endpoint (RFC 9540).
    pub fn gateway_url(&self) -> &str {
        &self.gateway_url
    }

    /// Target base URL, on a different origin than the gateway.
    pub fn target_url(&self) -> &str {
        &self.target_url
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Unblock each listener's accept() so the server threads can exit.
        for addr in &self.server_addrs {
            let _ = TcpStream::connect(addr);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
