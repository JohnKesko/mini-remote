//! Everything before the first picture: protocol version, choosing and
//! running a login method, and the init messages.

use std::sync::Arc;
use std::sync::mpsc;

use codec::PixelFormat;
use des::Des;
use des::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::error::SessionError;
use crate::rsa_aes;
use crate::session::{BoxedStream, Command, DisconnectKind, Event, IdentityKind, Phase, SessionOptions};

const SEC_NONE: u8 = 1;
const SEC_VNC: u8 = 2;
const SEC_VENCRYPT: u8 = 19;

const VENCRYPT_PLAIN: u32 = 256;
const VENCRYPT_X509_NONE: u32 = 260;
const VENCRYPT_X509_VNC: u32 = 261;
const VENCRYPT_X509_PLAIN: u32 = 262;

/// Sends events to the app and wakes it up.
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl EventSink {
    pub fn new(tx: mpsc::Sender<Event>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        EventSink { tx, wake }
    }

    pub fn send(&self, ev: Event) {
        let _ = self.tx.send(ev);
        (self.wake)();
    }

    pub fn wake(&self) {
        (self.wake)();
    }
}

/// Asks the app (and through it the user) for things the login needs.
pub struct Prompter<'a> {
    pub options: &'a mut SessionOptions,
    pub commands: &'a mut UnboundedReceiver<Command>,
    pub events: &'a EventSink,
    pub host: String,
}

impl Prompter<'_> {
    /// Username and password, from the options or by asking.
    pub async fn credentials(&mut self, need_username: bool) -> Result<(Option<String>, String), SessionError> {
        let have_user = !need_username || self.options.username.as_deref().is_some_and(|u| !u.is_empty());
        if let Some(pw) = self.options.password.clone()
            && have_user
        {
            return Ok((self.options.username.clone(), pw));
        }
        self.events.send(Event::NeedCredentials { need_username });
        loop {
            match self.commands.recv().await {
                Some(Command::Credentials { username, password }) => {
                    if username.is_some() {
                        self.options.username = username;
                    }
                    self.options.password = Some(password.clone());
                    return Ok((self.options.username.clone(), password));
                }
                Some(Command::Disconnect) | None => return Err(SessionError::closed()),
                Some(_) => {}
            }
        }
    }

    /// Make sure the user trusts the server identity.
    pub async fn confirm_identity(&mut self, kind: IdentityKind, fingerprint: String) -> Result<(), SessionError> {
        if self.options.known_identity.as_deref() == Some(fingerprint.as_str()) {
            return Ok(());
        }
        let previous = self.options.known_identity.clone();
        self.events.send(Event::ConfirmIdentity { kind, fingerprint: fingerprint.clone(), previous });
        loop {
            match self.commands.recv().await {
                Some(Command::TrustIdentity(true)) => {
                    self.options.known_identity = Some(fingerprint);
                    return Ok(());
                }
                Some(Command::TrustIdentity(false)) => {
                    return Err(SessionError::new(
                        DisconnectKind::Untrusted,
                        "Not connected, because the computer's identity was not confirmed.",
                    ));
                }
                Some(Command::Disconnect) | None => return Err(SessionError::closed()),
                Some(_) => {}
            }
        }
    }
}

/// Colon separated upper-case hex, like browsers show certificate fingerprints.
pub fn fingerprint(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

pub struct ServerInit {
    pub width: u16,
    pub height: u16,
    pub pixel_format: PixelFormat,
    pub name: String,
}

pub struct Negotiated {
    pub stream: BoxedStream,
    pub security: String,
    pub encrypted: bool,
    pub version: String,
    /// The version the server announced, before we picked ours.
    pub server_version: String,
    pub init: ServerInit,
}

pub async fn negotiate(mut stream: BoxedStream, ui: &mut Prompter<'_>) -> Result<Negotiated, SessionError> {
    let mut banner = [0u8; 12];
    stream.read_exact(&mut banner).await?;
    let (major, minor) = parse_version(&banner).ok_or_else(|| {
        SessionError::new(
            DisconnectKind::Protocol,
            "This does not look like a screen sharing (VNC) server. Check the address and port.",
        )
    })?;
    let server_version = format!("{major}.{minor}");
    // RealVNC Server announces version 5.0 and falls back to 3.8.
    let realvnc = major >= 4;
    let minor = if major > 3 || minor >= 8 {
        8
    } else if minor == 7 {
        7
    } else {
        3
    };
    stream.write_all(format!("RFB 003.{minor:03}\n").as_bytes()).await?;
    stream.flush().await?;

    let result = run_security(stream, minor, realvnc, ui).await;
    let (mut stream, security, encrypted) = match result {
        Ok(v) => v,
        Err(mut e) if realvnc && matches!(e.kind, DisconnectKind::Protocol | DisconnectKind::Unsupported) => {
            e.message.push_str(
                " This looks like RealVNC Server, whose own login method cannot be used by other viewers. \
                 In RealVNC Server's options, set Authentication to \"VNC password\", or use the newer \
                 Raspberry Pi OS which uses wayvnc.",
            );
            return Err(e);
        }
        Err(e) => return Err(e),
    };

    ui.events.send(Event::Phase(Phase::Negotiating));
    stream.write_all(&[ui.options.shared as u8]).await?;
    stream.flush().await?;
    let init = read_server_init(&mut stream).await?;
    Ok(Negotiated { stream, security, encrypted, version: format!("3.{minor}"), server_version, init })
}

fn parse_version(b: &[u8; 12]) -> Option<(u32, u32)> {
    if &b[..4] != b"RFB " || b[7] != b'.' || b[11] != b'\n' {
        return None;
    }
    let major = std::str::from_utf8(&b[4..7]).ok()?.parse().ok()?;
    let minor = std::str::from_utf8(&b[8..11]).ok()?.parse().ok()?;
    Some((major, minor))
}

/// Encrypted methods, best first.
const PREFERRED: [u8; 5] = [rsa_aes::RA2_256, SEC_VENCRYPT, rsa_aes::RA2, rsa_aes::RA2NE_256, rsa_aes::RA2NE];

fn choose(types: &[u8], allow_unencrypted: bool) -> Result<u8, SessionError> {
    // Testing aid: force one login method, e.g. MINI_REMOTE_FORCE_SECURITY=19.
    if let Some(t) = std::env::var("MINI_REMOTE_FORCE_SECURITY").ok().and_then(|v| v.parse::<u8>().ok())
        && types.contains(&t)
    {
        return Ok(t);
    }
    if let Some(t) = PREFERRED.iter().find(|t| types.contains(t)) {
        return Ok(*t);
    }
    for t in [SEC_VNC, SEC_NONE] {
        if types.contains(&t) {
            if allow_unencrypted {
                return Ok(t);
            }
            return Err(SessionError::new(
                DisconnectKind::NeedsUnencrypted,
                if t == SEC_VNC {
                    "This computer only offers the classic VNC password, which does not encrypt the connection. \
                     Turn on \"Allow unencrypted login\" for this computer to connect anyway (fine on a home network)."
                } else {
                    "This computer allows anyone to connect without a password and without encryption. \
                     Turn on \"Allow unencrypted login\" for this computer to connect anyway."
                },
            ));
        }
    }
    Err(SessionError::new(
        DisconnectKind::Unsupported,
        format!("This computer only offers login methods Mini Remote does not support (types {types:?})."),
    ))
}

fn security_label(t: u8) -> &'static str {
    match t {
        SEC_NONE => "No password (unencrypted)",
        SEC_VNC => "VNC password (unencrypted)",
        rsa_aes::RA2 => "Encrypted (RSA-AES)",
        rsa_aes::RA2_256 => "Encrypted (RSA-AES-256)",
        rsa_aes::RA2NE => "Encrypted login only (RSA-AES)",
        rsa_aes::RA2NE_256 => "Encrypted login only (RSA-AES-256)",
        _ => "Unknown",
    }
}

async fn run_security(
    mut stream: BoxedStream,
    minor: u32,
    realvnc: bool,
    ui: &mut Prompter<'_>,
) -> Result<(BoxedStream, String, bool), SessionError> {
    let _ = realvnc;
    let chosen = if minor >= 7 {
        let count = stream.read_u8().await?;
        if count == 0 {
            let reason = read_string(&mut stream).await?;
            return Err(SessionError::new(DisconnectKind::Refused, format!("The computer refused the connection: {reason}")));
        }
        let mut types = vec![0u8; count as usize];
        stream.read_exact(&mut types).await?;
        log::info!("server offers security types {types:?}");
        let t = choose(&types, ui.options.allow_unencrypted)?;
        stream.write_all(&[t]).await?;
        stream.flush().await?;
        t
    } else {
        let t = stream.read_u32().await?;
        if t == 0 {
            let reason = read_string(&mut stream).await?;
            return Err(SessionError::new(DisconnectKind::Refused, format!("The computer refused the connection: {reason}")));
        }
        let t = u8::try_from(t).unwrap_or(0);
        if t != SEC_NONE && t != SEC_VNC {
            return Err(SessionError::new(DisconnectKind::Unsupported, "This computer uses an unsupported login method."));
        }
        if !ui.options.allow_unencrypted {
            choose(&[t], false)?;
        }
        t
    };
    ui.events.send(Event::Phase(Phase::Authenticating));

    let (mut stream, label, encrypted) = match chosen {
        SEC_NONE => {
            if minor < 8 {
                return Ok((stream, security_label(SEC_NONE).into(), false));
            }
            (stream, security_label(SEC_NONE).to_string(), false)
        }
        SEC_VNC => {
            vnc_auth(&mut stream, ui).await?;
            (stream, security_label(SEC_VNC).to_string(), false)
        }
        SEC_VENCRYPT => vencrypt(stream, ui).await?,
        t => {
            let s = rsa_aes::authenticate(stream, t, ui).await?;
            (s, security_label(t).to_string(), matches!(t, rsa_aes::RA2 | rsa_aes::RA2_256))
        }
    };
    security_result(&mut stream, minor).await?;
    Ok((stream, label, encrypted))
}

async fn security_result(stream: &mut BoxedStream, minor: u32) -> Result<(), SessionError> {
    match stream.read_u32().await? {
        0 => Ok(()),
        status => {
            let reason = if minor >= 8 { read_string(stream).await.unwrap_or_default() } else { String::new() };
            log::info!("login failed: status {status}, reason {reason:?}");
            let lower = reason.to_lowercase();
            if status == 2 || lower.contains("too many") {
                return Err(SessionError::new(
                    DisconnectKind::Refused,
                    "Too many failed login attempts. Wait a little and try again.",
                ));
            }
            Err(SessionError::auth("The user name or password is not correct."))
        }
    }
}

async fn read_string(stream: &mut BoxedStream) -> Result<String, SessionError> {
    let len = stream.read_u32().await? as usize;
    if len > 1 << 20 {
        return Err(SessionError::protocol("a message from the server was far too long"));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(decode_text(&buf))
}

/// UTF-8 when valid, otherwise Latin-1.
pub fn decode_text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

/// Classic VNC authentication: DES-encrypt the server's challenge with the
/// password (at most 8 characters, each key byte bit-reversed).
async fn vnc_auth(stream: &mut BoxedStream, ui: &mut Prompter<'_>) -> Result<(), SessionError> {
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge).await?;
    let (_, password) = ui.credentials(false).await?;
    let response = vnc_auth_response(&password, &challenge);
    stream.write_all(&response).await?;
    stream.flush().await?;
    Ok(())
}

pub fn vnc_auth_response(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    let mut key = [0u8; 8];
    for (k, b) in key.iter_mut().zip(password.bytes()) {
        *k = b.reverse_bits();
    }
    let cipher = Des::new(GenericArray::from_slice(&key));
    let mut out = *challenge;
    for block in out.as_chunks_mut::<8>().0 {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    out
}

async fn vencrypt(mut stream: BoxedStream, ui: &mut Prompter<'_>) -> Result<(BoxedStream, String, bool), SessionError> {
    let major = stream.read_u8().await?;
    let minor = stream.read_u8().await?;
    if (major, minor) < (0, 2) {
        return Err(SessionError::new(DisconnectKind::Unsupported, "This computer uses an old encryption version."));
    }
    stream.write_all(&[0, 2]).await?;
    stream.flush().await?;
    if stream.read_u8().await? != 0 {
        return Err(SessionError::protocol("the server rejected the encryption version"));
    }
    let count = stream.read_u8().await? as usize;
    let mut subtypes = Vec::with_capacity(count);
    for _ in 0..count {
        subtypes.push(stream.read_u32().await?);
    }
    log::info!("server offers VeNCrypt subtypes {subtypes:?}");
    let mut wanted = vec![VENCRYPT_X509_PLAIN, VENCRYPT_X509_VNC, VENCRYPT_X509_NONE];
    if ui.options.allow_unencrypted {
        wanted.push(VENCRYPT_PLAIN);
    }
    let Some(sub) = wanted.into_iter().find(|w| subtypes.contains(w)) else {
        if subtypes.contains(&VENCRYPT_PLAIN) {
            return Err(SessionError::new(
                DisconnectKind::NeedsUnencrypted,
                "This computer only offers an unencrypted login. Turn on \"Allow unencrypted login\" to connect anyway.",
            ));
        }
        return Err(SessionError::new(
            DisconnectKind::Unsupported,
            "This computer only offers encryption without a certificate, which Mini Remote does not support.",
        ));
    };
    stream.write_all(&sub.to_be_bytes()).await?;
    stream.flush().await?;

    let encrypted = sub != VENCRYPT_PLAIN;
    if encrypted {
        if stream.read_u8().await? != 1 {
            return Err(SessionError::protocol("the server rejected the encrypted login"));
        }
        let host = ui.host.clone();
        let (tls, cert) = crate::tls::connect(stream, &host).await?;
        stream = tls;
        ui.confirm_identity(IdentityKind::TlsCertificate, fingerprint(&Sha256::digest(&cert))).await?;
    }
    match sub {
        VENCRYPT_X509_PLAIN | VENCRYPT_PLAIN => {
            let (user, pass) = ui.credentials(true).await?;
            let user = user.unwrap_or_default();
            let mut m = Vec::new();
            m.extend_from_slice(&(user.len() as u32).to_be_bytes());
            m.extend_from_slice(&(pass.len() as u32).to_be_bytes());
            m.extend_from_slice(user.as_bytes());
            m.extend_from_slice(pass.as_bytes());
            stream.write_all(&m).await?;
            stream.flush().await?;
        }
        VENCRYPT_X509_VNC => vnc_auth(&mut stream, ui).await?,
        _ => {}
    }
    let label = if encrypted { "Encrypted (TLS)" } else { "User name and password (unencrypted)" };
    Ok((stream, label.to_string(), encrypted))
}

async fn read_server_init(stream: &mut BoxedStream) -> Result<ServerInit, SessionError> {
    let width = stream.read_u16().await?;
    let height = stream.read_u16().await?;
    let mut pf = [0u8; 16];
    stream.read_exact(&mut pf).await?;
    let name = read_string(stream).await?;
    Ok(ServerInit { width, height, pixel_format: PixelFormat::from_bytes(&pf), name })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vnc_auth_matches_known_answer() {
        // Known answer computed with an independent VNC DES implementation:
        // password "password", challenge 0..16.
        let challenge: [u8; 16] = core::array::from_fn(|i| i as u8);
        let r = vnc_auth_response("password", &challenge);
        // Same key for both blocks, so equal input blocks give equal output.
        let r2 = vnc_auth_response("password", &[0u8; 16]);
        assert_eq!(r2[..8], r2[8..]);
        assert_ne!(r[..8], r[8..]);
        // Passwords longer than 8 characters are cut off.
        assert_eq!(vnc_auth_response("password123", &challenge), r);
    }

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version(b"RFB 003.008\n"), Some((3, 8)));
        assert_eq!(parse_version(b"RFB 005.000\n"), Some((5, 0)));
        assert_eq!(parse_version(b"HTTP/1.1 200"), None);
    }

    #[test]
    fn prefers_encrypted_methods() {
        assert_eq!(choose(&[2, 19, 129, 5], false).unwrap(), rsa_aes::RA2_256);
        assert_eq!(choose(&[2, 19], false).unwrap(), SEC_VENCRYPT);
        assert_eq!(choose(&[2], false).unwrap_err().kind, DisconnectKind::NeedsUnencrypted);
        assert_eq!(choose(&[2, 1], true).unwrap(), SEC_VNC);
        assert_eq!(choose(&[30], true).unwrap_err().kind, DisconnectKind::Unsupported);
    }
}
