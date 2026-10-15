//! Prints harness URLs as one JSON line on stdout, then parks until killed.
//!
//! Used by the JS e2e test. Start with:
//! `cargo run --features harness --bin ohttp-test-harness`

use ohttp_client::harness::TestHarness;

fn main() {
    let h = TestHarness::start();
    // Prefix so consumers can ignore other stdout noise.
    println!(
        "OHTTP_HARNESS\t{{\"relay_url\":\"{}\",\"gateway_url\":\"{}\",\"target_url\":\"{}\"}}",
        h.relay_url(),
        h.gateway_url(),
        h.target_url(),
    );
    // Hold servers up until the parent process kills us.
    loop {
        std::thread::park();
    }
}
