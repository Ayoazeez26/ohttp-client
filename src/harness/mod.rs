//! End-to-end test harness: a target HTTP server, an OHTTP gateway serving
//! its key config, an OHTTP relay ([`ohttp_relay`]) in front of it, and a
//! generic HTTP `CONNECT` proxy standing in for a relay's key-fetch tunnel.
//!
//! The target and gateway are minimal hand-rolled HTTP/1.1 servers on
//! std-thread [`TcpListener`]s; the relay runs on a background tokio runtime.
//! The `CONNECT` proxy is a second hand-rolled server: `ohttp-relay`'s own
//! CONNECT bootstrap gates on an HTTPS-only gateway opt-in probe that our
//! plain-HTTP test gateway can't satisfy, but `CONNECT` tunneling itself is a
//! generic HTTP mechanism, so a minimal standalone proxy exercises the same
//! client-side behavior. Everything shuts down gracefully when the
//! [`TestHarness`] is dropped.

mod connect_proxy;
mod gateway;
mod server;

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use connect_proxy::serve_connect_proxy;
use gateway::handle_gateway_request;
use server::serve;

const GATEWAY_PATH: &str = "/.well-known/ohttp-gateway";

pub struct TestHarness {
    relay_url: String,
    gateway_url: String,
    target_url: String,
    connect_proxy_url: String,
    shutdown: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    server_addrs: Vec<SocketAddr>,
    relay_rt: Option<tokio::runtime::Runtime>,
}

impl TestHarness {
    pub fn start() -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut server_addrs = Vec::new();

        // Target: the resource the inner request is addressed to. It echoes
        // the request line and body so the test can assert what came through.
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

        // Gateway: serves its key config, opts in to relaying, and
        // decapsulates requests, forwarding them to the inner target.
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

        // Relay: the real ohttp-relay crate, forwarding to the gateway.
        // `free_port` + bind is racy under parallel tests; retry on EADDRINUSE.
        let gateway_uri: ohttp_relay::GatewayUri =
            format!("http://127.0.0.1:{}", gateway_addr.port())
                .parse()
                .unwrap();
        let relay_rt = tokio::runtime::Runtime::new().unwrap();
        let relay_port = {
            let mut last_err = None;
            let mut bound = None;
            for _ in 0..16 {
                let port = free_port();
                match relay_rt.block_on(ohttp_relay::listen_tcp(port, gateway_uri.clone())) {
                    Ok(_handle) => {
                        bound = Some(port);
                        break;
                    }
                    Err(err) => last_err = Some(err),
                }
            }
            bound.unwrap_or_else(|| {
                panic!("failed to bind ohttp-relay after retries: {:?}", last_err)
            })
        };

        // Generic CONNECT proxy: stands in for a relay's key-fetch tunnel.
        let connect_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let connect_addr = connect_listener.local_addr().unwrap();
        server_addrs.push(connect_addr);
        threads.push(serve_connect_proxy(connect_listener, shutdown.clone()));

        TestHarness {
            relay_url: format!("http://127.0.0.1:{relay_port}/"),
            gateway_url: format!("http://127.0.0.1:{}{}", gateway_addr.port(), GATEWAY_PATH),
            target_url: format!("http://127.0.0.1:{}", target_addr.port()),
            connect_proxy_url: format!("http://127.0.0.1:{}/", connect_addr.port()),
            shutdown,
            threads,
            server_addrs,
            relay_rt: Some(relay_rt),
        }
    }

    /// URL the encapsulated request is POSTed to.
    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }

    /// RFC 9540 gateway endpoint; GET it to fetch the key config.
    pub fn gateway_url(&self) -> &str {
        &self.gateway_url
    }

    /// Base URL of the target resource (distinct from the gateway).
    pub fn target_url(&self) -> &str {
        &self.target_url
    }

    /// URL of a generic HTTP `CONNECT` proxy, standing in for a relay's
    /// key-fetch tunnel.
    pub fn connect_proxy_url(&self) -> &str {
        &self.connect_proxy_url
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
        if let Some(rt) = self.relay_rt.take() {
            rt.shutdown_background();
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
