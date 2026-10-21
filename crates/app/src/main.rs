//! Mini Remote: see and control a Raspberry Pi (or another computer with
//! screen sharing) from this computer.
//!
//! Usage: `mini-remote [--connect host[:port]]`

// Buttons drawn inside a `match` arm read better than buttons hidden in match guards.
#![allow(clippy::collapsible_match)]

mod app;
mod clipboard;
mod files;
mod input;
mod model;
mod platform;
mod remote_login;
mod runner;
mod session;
mod storage;
mod texture;
mod ui;
mod updater;

fn main() {
    init_logging();
    let mut connect = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--connect" | "-c" => connect = args.next(),
            "--help" | "-h" => {
                println!("Usage: mini-remote [--connect host[:port]]");
                return;
            }
            other if other.starts_with("--connect=") => {
                connect = other.strip_prefix("--connect=").map(str::to_string);
            }
            // macOS adds this when started from Finder.
            other if other.starts_with("-psn_") => {}
            other => log::warn!("ignoring unknown argument {other}"),
        }
    }
    if let Err(err) = runner::run(connect) {
        log::error!("{err}");
        eprintln!("Mini Remote could not start: {err}");
        std::process::exit(1);
    }
}

/// A tiny logger writing to stderr. `MINI_REMOTE_LOG=debug` shows more.
fn init_logging() {
    struct Logger {
        level: log::LevelFilter,
    }
    impl log::Log for Logger {
        fn enabled(&self, meta: &log::Metadata) -> bool {
            let ours = meta.target().starts_with("mini_remote")
                || meta.target().starts_with("rfb")
                || meta.target().starts_with("codec")
                || meta.target().starts_with("ssh")
                || meta.target().starts_with("discovery");
            let limit = if ours { self.level } else { self.level.min(log::LevelFilter::Warn) };
            meta.level() <= limit
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                eprintln!("[{} {}] {}", record.level(), record.target(), record.args());
            }
        }
        fn flush(&self) {}
    }
    let level = std::env::var("MINI_REMOTE_LOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(log::LevelFilter::Info);
    if log::set_boxed_logger(Box::new(Logger { level })).is_ok() {
        log::set_max_level(level.max(log::LevelFilter::Warn));
    }
}
