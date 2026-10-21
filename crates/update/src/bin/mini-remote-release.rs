//! Maintainer tool for signing releases.
//!
//!   mini-remote-release keygen [--key PATH]
//!       Make a new signing key (refuses to overwrite one). Prints the public key
//!       to put in crates/update/src/lib.rs (PUBLIC_KEY_HEX).
//!   mini-remote-release public-key [--key PATH]
//!   mini-remote-release sign --version 1.2.3 [--key PATH] FILE...
//!       Writes FILE.sig next to each file.
//!   mini-remote-release verify --version 1.2.3 FILE...
//!       Checks FILE.sig against the public key built into the app.
//!
//! The key lives outside the project, by default in
//! ~/.config/mini-remote/release-signing-key. Keep a backup: without it,
//! installed copies can't be updated automatically any more.

use std::path::{Path, PathBuf};
use std::process::exit;

use ed25519_dalek::{Signer, SigningKey};
use rand::RngCore;

fn default_key_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("mini-remote").join("release-signing-key")
}

fn fail(message: impl AsRef<str>) -> ! {
    eprintln!("error: {}", message.as_ref());
    exit(1);
}

fn load_key(path: &Path) -> SigningKey {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| fail(format!("can't read the signing key at {}: {e}", path.display())));
    let seed = update::from_hex::<32>(&text).unwrap_or_else(|| fail("the signing key file is not valid"));
    SigningKey::from_bytes(&seed)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut key_path = default_key_path();
    let mut version = None;
    let mut files = Vec::new();
    let mut command = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--key" => {
                i += 1;
                key_path = PathBuf::from(args.get(i).unwrap_or_else(|| fail("--key needs a path")));
            }
            "--version" => {
                i += 1;
                version = Some(args.get(i).unwrap_or_else(|| fail("--version needs a value")).clone());
            }
            other if command.is_none() => command = Some(other.to_string()),
            other => files.push(PathBuf::from(other)),
        }
        i += 1;
    }

    match command.as_deref() {
        Some("keygen") => {
            if key_path.exists() {
                fail(format!("a key already exists at {}; not overwriting it", key_path.display()));
            }
            let mut seed = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut seed);
            if let Some(dir) = key_path.parent() {
                std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(e.to_string()));
            }
            write_private(&key_path, &update::to_hex(&seed));
            let key = SigningKey::from_bytes(&seed);
            println!("New signing key saved to {}", key_path.display());
            println!("Public key: {}", update::to_hex(key.verifying_key().as_bytes()));
        }
        Some("public-key") => {
            println!("{}", update::to_hex(load_key(&key_path).verifying_key().as_bytes()));
        }
        Some("sign") => {
            let version = version.unwrap_or_else(|| fail("sign needs --version"));
            let key = load_key(&key_path);
            for file in &files {
                let name = file_name(file);
                let contents = std::fs::read(file).unwrap_or_else(|e| fail(format!("{}: {e}", file.display())));
                let signature = key.sign(&update::signed_message(&version, &name, &contents));
                let sig_path = file.with_file_name(format!("{name}.sig"));
                std::fs::write(&sig_path, update::to_hex(&signature.to_bytes()) + "\n")
                    .unwrap_or_else(|e| fail(e.to_string()));
                println!("signed {}", sig_path.display());
            }
        }
        Some("verify") => {
            let version = version.unwrap_or_else(|| fail("verify needs --version"));
            let mut ok = true;
            for file in &files {
                let name = file_name(file);
                let contents = std::fs::read(file).unwrap_or_else(|e| fail(format!("{}: {e}", file.display())));
                let sig = std::fs::read_to_string(file.with_file_name(format!("{name}.sig"))).unwrap_or_default();
                let good = update::verify(&version, &name, &contents, &sig);
                println!("{}: {}", name, if good { "good signature" } else { "BAD signature" });
                ok &= good;
            }
            if !ok {
                exit(1);
            }
        }
        _ => fail("usage: mini-remote-release keygen | public-key | sign --version V FILE... | verify --version V FILE..."),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().and_then(|n| n.to_str()).unwrap_or_else(|| fail("bad file name")).to_string()
}

/// Write a file only the owner can read.
fn write_private(path: &Path, text: &str) {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap_or_else(|e| fail(e.to_string()));
        f.write_all(text.as_bytes()).unwrap_or_else(|e| fail(e.to_string()));
    }
    #[cfg(not(unix))]
    std::fs::write(path, text).unwrap_or_else(|e| fail(e.to_string()));
}
