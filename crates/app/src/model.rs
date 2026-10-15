//! Saved connections and app settings, as stored in the JSON files.

use rfb::Quality;
use serde::{Deserialize, Serialize};

pub const DEFAULT_VNC_PORT: u16 = 5900;
pub const DEFAULT_SSH_PORT: u16 = 22;

/// How the remote picture fits into the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scaling {
    #[default]
    Fit,
    /// One remote pixel per screen pixel.
    Actual,
}

/// Send typed characters (works with any keyboard layout on the Pi) or the
/// position of each key (the Pi's own layout decides the character).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyboardMode {
    #[default]
    Characters,
    Physical,
}

/// What the Command key (Windows key elsewhere) sends to the remote computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetaMapping {
    Control,
    Super,
}

/// What the Option key (Alt elsewhere) sends to the remote computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AltMapping {
    Alt,
    AltGr,
}

/// How much room the side menu takes while a remote screen is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidebarMode {
    #[default]
    Full,
    /// Icons only.
    Rail,
    Hidden,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// How to log in for secure remote login (SSH).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SshLogin {
    /// The same password as the screen sharing login.
    #[default]
    SamePassword,
    /// A separate password, kept in the password store.
    Password,
    /// A private key file on this computer (its passphrase, if any, is in the password store).
    KeyFile(String),
    /// Keys held by the SSH agent.
    Agent,
}

/// Secure remote login (SSH): used for file transfer, and optionally to carry
/// the screen connection through an encrypted tunnel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshSettings {
    /// Send the screen connection through an SSH tunnel.
    pub enabled: bool,
    /// Empty means the same address as the screen sharing connection.
    pub host: String,
    pub port: u16,
    /// Empty means the same username as the screen sharing login.
    pub username: String,
    pub login: SshLogin,
}

impl Default for SshSettings {
    fn default() -> Self {
        SshSettings {
            enabled: false,
            host: String::new(),
            port: DEFAULT_SSH_PORT,
            username: String::new(),
            login: SshLogin::SamePassword,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    /// Stable id, also used for the Keychain entry and the thumbnail file.
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// The password is in the system password store (never in files).
    pub remember_password: bool,
    pub quality: Quality,
    pub video: bool,
    pub scaling: Scaling,
    pub resize_remote: bool,
    pub view_only: bool,
    pub share_clipboard: bool,
    pub keyboard_mode: KeyboardMode,
    pub allow_unencrypted: bool,
    pub ssh: SshSettings,
    /// Seconds since 1970 of the last successful connection.
    pub last_used: Option<i64>,
}

impl Default for Connection {
    fn default() -> Self {
        Connection {
            id: String::new(),
            name: String::new(),
            host: String::new(),
            port: DEFAULT_VNC_PORT,
            username: String::new(),
            remember_password: true,
            quality: Quality::Automatic,
            video: false,
            scaling: Scaling::Fit,
            resize_remote: false,
            view_only: false,
            share_clipboard: true,
            keyboard_mode: KeyboardMode::Characters,
            allow_unencrypted: false,
            ssh: SshSettings::default(),
            last_used: None,
        }
    }
}

impl Connection {
    /// A new connection using the defaults from the settings.
    pub fn new_with_defaults(settings: &Settings) -> Self {
        Connection {
            id: new_id(),
            quality: settings.default_quality,
            video: settings.default_video,
            allow_unencrypted: settings.default_allow_unencrypted,
            keyboard_mode: settings.keyboard_mode,
            share_clipboard: settings.share_clipboard,
            ..Connection::default()
        }
    }

    /// Name to show: the given name, or the address.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() { &self.host } else { self.name.trim() }
    }

    /// "raspberrypi.local" or "raspberrypi.local:5901".
    pub fn address_label(&self) -> String {
        if self.port == DEFAULT_VNC_PORT {
            self.host.clone()
        } else if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Key for the trusted identity list.
    pub fn host_key(&self) -> String {
        format!("{}:{}", self.host.to_lowercase(), self.port)
    }

    /// Key for the password store.
    pub fn secret_key(&self) -> String {
        format!("connection-{}", self.id)
    }

    /// Key for the SSH password (or key passphrase) in the password store.
    pub fn ssh_secret_key(&self) -> String {
        format!("connection-{}-ssh", self.id)
    }

    /// Address for secure remote login.
    pub fn ssh_host(&self) -> &str {
        let h = self.ssh.host.trim();
        if h.is_empty() { self.host.trim() } else { h }
    }

    /// Username for secure remote login.
    pub fn ssh_username(&self) -> &str {
        let u = self.ssh.username.trim();
        if u.is_empty() { self.username.trim() } else { u }
    }

    /// Key for the trusted identity list (SSH identities are kept apart).
    pub fn ssh_host_key(&self) -> String {
        format!("ssh:{}:{}", self.ssh_host().to_lowercase(), self.ssh.port)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    pub meta_key: MetaMapping,
    pub alt_key: AltMapping,
    /// Default for new connections.
    pub keyboard_mode: KeyboardMode,
    /// Master switch; each connection can also turn it off.
    pub share_clipboard: bool,
    /// Default for new connections.
    pub default_quality: Quality,
    /// Default for new connections: prefer the H.264 video encoding.
    pub default_video: bool,
    /// Default for new connections: allow logins without encryption.
    pub default_allow_unencrypted: bool,
    pub auto_reconnect: bool,
    /// Where copied files go; `None` means the Downloads folder.
    pub download_dir: Option<String>,
    /// Side menu size while a remote screen is shown.
    pub sidebar: SidebarMode,
}

impl Settings {
    /// Folder for files copied from the remote computer.
    pub fn download_folder(&self) -> std::path::PathBuf {
        self.download_dir
            .as_ref()
            .filter(|d| !d.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(crate::platform::download_dir)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            appearance: Appearance::System,
            meta_key: crate::platform::default_meta_mapping(),
            alt_key: crate::platform::default_alt_mapping(),
            keyboard_mode: KeyboardMode::Characters,
            share_clipboard: true,
            default_quality: Quality::Automatic,
            default_video: false,
            default_allow_unencrypted: false,
            auto_reconnect: true,
            download_dir: None,
            sidebar: SidebarMode::Full,
        }
    }
}

/// A random-enough unique id without extra dependencies.
pub fn new_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(nanos);
    format!("{:016x}", hasher.finish())
}

/// Seconds since 1970.
pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Turn what the user typed into a host and port.
///
/// Accepts `host`, `host:port`, `[ipv6]:port`, a bare IPv6 address,
/// `vnc://host:port`, and the classic VNC `host:1` (display 1 = port 5901).
pub fn parse_address(input: &str) -> Option<(String, u16)> {
    let mut text = input.trim();
    if let Some(rest) = text.strip_prefix("vnc://") {
        text = rest;
    }
    let text = text.trim_end_matches('/');
    if text.is_empty() {
        return None;
    }

    let port_from = |p: &str| -> Option<u16> {
        let n: u32 = p.parse().ok()?;
        // Small numbers are VNC display numbers.
        if n < 100 { u16::try_from(DEFAULT_VNC_PORT as u32 + n).ok() } else { u16::try_from(n).ok() }
    };

    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => port_from(p)?,
            None if after.is_empty() => DEFAULT_VNC_PORT,
            None => return None,
        };
        return (!host.is_empty()).then(|| (host.to_string(), port));
    }

    if text.parse::<std::net::Ipv6Addr>().is_ok() {
        return Some((text.to_string(), DEFAULT_VNC_PORT));
    }

    // "host::5901" is the classic way to give a real port number.
    if let Some((host, port)) = text.split_once("::")
        && !host.contains(':')
        && !host.is_empty()
    {
        let port: u16 = port.parse().ok()?;
        return Some((host.to_string(), port));
    }

    match text.matches(':').count() {
        0 => Some((text.to_string(), DEFAULT_VNC_PORT)),
        1 => {
            let (host, port) = text.split_once(':')?;
            if host.is_empty() {
                return None;
            }
            Some((host.to_string(), port_from(port)?))
        }
        // A bare IPv6 address.
        _ => Some((text.to_string(), DEFAULT_VNC_PORT)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(parse_address("raspberrypi.local"), Some(("raspberrypi.local".into(), 5900)));
        assert_eq!(parse_address(" pi:5901 "), Some(("pi".into(), 5901)));
        assert_eq!(parse_address("pi:1"), Some(("pi".into(), 5901)));
        assert_eq!(parse_address("pi::5902"), Some(("pi".into(), 5902)));
        assert_eq!(parse_address("vnc://10.0.0.5:5900/"), Some(("10.0.0.5".into(), 5900)));
        assert_eq!(parse_address("[fe80::1]:5905"), Some(("fe80::1".into(), 5905)));
        assert_eq!(parse_address("fe80::1"), Some(("fe80::1".into(), 5900)));
        assert_eq!(parse_address("pi:abc"), None);
        assert_eq!(parse_address(""), None);
        assert_eq!(parse_address(":5900"), None);
    }

    #[test]
    fn labels() {
        let mut c = Connection { host: "pi".into(), ..Connection::default() };
        assert_eq!(c.address_label(), "pi");
        assert_eq!(c.display_name(), "pi");
        c.port = 5901;
        assert_eq!(c.address_label(), "pi:5901");
        c.name = "Living room".into();
        assert_eq!(c.display_name(), "Living room");
    }
}
