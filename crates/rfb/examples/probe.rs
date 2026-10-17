//! Command line test tool: connect, log in, watch the screen for a while,
//! optionally type some text, and save the final picture as a PNG.
//!
//! cargo run -p mini-remote-rfb --example probe -- HOST:PORT [options]
//!   --user NAME --password PASS   login (or env VNC_USER / VNC_PASSWORD)
//!   --unencrypted                 allow unencrypted login methods
//!   --quality best|balanced|fast  picture quality
//!   --video                       prefer H.264 video
//!   --type TEXT                   type TEXT (then Enter) after connecting
//!   --paste                       press Ctrl+V after typing
//!   --paste-terminal              press Ctrl+Shift+V after typing (how a terminal pastes)
//!   --copy-all                    press Ctrl+A, Ctrl+C after typing (remote clipboard -> event)
//!   --click X,Y                   where to click before typing (default 960,540)
//!   --clipboard TEXT              send TEXT to the remote clipboard
//!   --resize WxH                  ask the server to change its screen size
//!   --seconds N                   how long to stay connected (default 4)
//!   --out FILE.png                where to save the picture

use std::sync::Arc;
use std::time::{Duration, Instant};

use rfb::{Command, Event, Quality, SessionOptions};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(target) = args.first().cloned() else {
        eprintln!("usage: probe HOST:PORT [options]");
        std::process::exit(2);
    };
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let (host, port) = match target.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().expect("port")),
        None => (target.clone(), 5900),
    };
    let quality = match opt("--quality").as_deref() {
        Some("best") => Quality::Best,
        Some("balanced") => Quality::Balanced,
        Some("fast") => Quality::Fast,
        _ => Quality::Automatic,
    };
    let options = SessionOptions {
        host_label: target.clone(),
        username: opt("--user").or_else(|| std::env::var("VNC_USER").ok()),
        password: opt("--password").or_else(|| std::env::var("VNC_PASSWORD").ok()),
        allow_unencrypted: flag("--unencrypted"),
        quality,
        video: flag("--video"),
        ..Default::default()
    };
    let seconds: u64 = opt("--seconds").and_then(|s| s.parse().ok()).unwrap_or(4);
    let out = opt("--out").unwrap_or_else(|| "probe.png".into());

    let rt = tokio::runtime::Runtime::new().unwrap();
    let session = rfb::start(rt.handle(), rfb::connect_tcp(host, port), options, Arc::new(|| {}));
    let started = Instant::now();
    let mut connected_at = None;
    let mut typed = false;
    loop {
        while let Some(ev) = session.try_event() {
            match ev {
                Event::ConfirmIdentity { kind, fingerprint, previous } => {
                    println!("identity {kind:?} {fingerprint} (previous {previous:?}) -> trusting for this test");
                    session.send(Command::TrustIdentity(true));
                }
                Event::NeedCredentials { need_username } => {
                    println!("server needs credentials (username: {need_username}) but none were given");
                    session.send(Command::Disconnect);
                }
                Event::Connected(info) => {
                    println!("CONNECTED {info:?}");
                    connected_at = Some(Instant::now());
                    if let Some(text) = opt("--clipboard") {
                        session.send(Command::Clipboard(text));
                    }
                    if let Some(size) = opt("--resize")
                        && let Some((w, h)) = size.split_once('x')
                    {
                        session.send(Command::SetDesktopSize { width: w.parse().unwrap(), height: h.parse().unwrap() });
                    }
                }
                Event::Stats(s) => println!("stats {s:?}"),
                Event::Disconnected(r) => {
                    println!("DISCONNECTED {:?}: {}", r.kind, r.message);
                    save(&session, &out);
                    return;
                }
                Event::Cursor(c) => println!("cursor {:?}", c.map(|c| (c.width, c.height, c.hotspot_x, c.hotspot_y))),
                other => println!("event {other:?}"),
            }
        }
        if let Some(t) = connected_at {
            if !typed && t.elapsed() > Duration::from_millis(800) {
                typed = true;
                if let Some(text) = opt("--type") {
                    let (x, y) = opt("--click")
                        .and_then(|c| c.split_once(',').map(|(x, y)| (x.parse().unwrap(), y.parse().unwrap())))
                        .unwrap_or((960, 540));
                    type_text(&session, &text, x, y);
                }
                if flag("--paste") {
                    combo(&session, 0x76);
                }
                if flag("--paste-terminal") {
                    // Same keys Mini Remote sends for Command+Shift+V.
                    let v = std::env::var("PROBE_V_KEYSYM").ok().and_then(|h| u32::from_str_radix(&h, 16).ok()).unwrap_or(0x76);
                    for (down, keysym) in [(true, 0xffe3), (true, 0xffe1), (true, v), (false, v), (false, 0xffe1), (false, 0xffe3)] {
                        session.send(Command::Key { down, keysym, scancode: 0 });
                    }
                }
                if flag("--copy-all") {
                    combo(&session, 0x61);
                    std::thread::sleep(Duration::from_millis(200));
                    combo(&session, 0x63);
                }
            }
            if t.elapsed() > Duration::from_secs(seconds) {
                save(&session, &out);
                session.send(Command::Disconnect);
                std::thread::sleep(Duration::from_millis(300));
                return;
            }
        } else if started.elapsed() > Duration::from_secs(30) {
            println!("TIMEOUT waiting for the first picture");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn type_text(session: &rfb::SessionHandle, text: &str, x: u16, y: u16) {
    // Click so the window under the pointer has keyboard focus.
    session.send(Command::Pointer { x, y, buttons: rfb::buttons::LEFT });
    session.send(Command::Pointer { x, y, buttons: 0 });
    std::thread::sleep(Duration::from_millis(200));
    for c in text.chars() {
        let keysym = if (c as u32) < 0x100 { c as u32 } else { 0x0100_0000 + c as u32 };
        session.send(Command::Key { down: true, keysym, scancode: 0 });
        session.send(Command::Key { down: false, keysym, scancode: 0 });
        std::thread::sleep(Duration::from_millis(15));
    }
    session.send(Command::Key { down: true, keysym: 0xff0d, scancode: 0 });
    session.send(Command::Key { down: false, keysym: 0xff0d, scancode: 0 });
}

/// Control + key.
fn combo(session: &rfb::SessionHandle, keysym: u32) {
    session.send(Command::Key { down: true, keysym: 0xffe3, scancode: 0 });
    session.send(Command::Key { down: true, keysym, scancode: 0 });
    session.send(Command::Key { down: false, keysym, scancode: 0 });
    session.send(Command::Key { down: false, keysym: 0xffe3, scancode: 0 });
}

fn save(session: &rfb::SessionHandle, path: &str) {
    let f = session.frame().lock();
    if f.width == 0 {
        return;
    }
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), f.width as u32, f.height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&f.pixels).unwrap();
    println!("saved {path} ({}x{})", f.width, f.height);
}
