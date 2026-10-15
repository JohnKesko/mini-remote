//! Runs one discovery and prints what it finds as it happens.
//!
//! `cargo run -p mini-remote-discovery --example scan`
//!
//! Set `RUST_LOG=debug` to also see what the individual methods are doing.

use std::time::Instant;

use discovery::{DiscoveredHost, DiscoveryEvent, DiscoveryOptions, discover};

fn describe(host: &DiscoveredHost) -> String {
    format!(
        "{} name={} vnc={:?} rfb={} ssh={} pi={} service={}",
        host.address,
        host.hostname.as_deref().unwrap_or("-"),
        host.vnc_ports,
        host.rfb_version.as_deref().unwrap_or("-"),
        host.ssh,
        host.is_raspberry_pi,
        host.service_name.as_deref().unwrap_or("-"),
    )
}

fn main() {
    env_logger::init();
    let runtime = tokio::runtime::Runtime::new().expect("could not start the async runtime");
    runtime.block_on(async {
        let started = Instant::now();
        let mut events = discover(DiscoveryOptions::default());
        while let Some(event) = events.recv().await {
            let seconds = started.elapsed().as_secs_f32();
            match event {
                DiscoveryEvent::Found(host) => {
                    println!("[{seconds:5.2}s] found   {}", describe(&host))
                }
                DiscoveryEvent::Updated(host) => {
                    println!("[{seconds:5.2}s] updated {}", describe(&host))
                }
                DiscoveryEvent::Finished => println!("[{seconds:5.2}s] finished"),
            }
        }
    });
    // Do not wait for slow name lookups that are still running in the background.
    runtime.shutdown_background();
}
